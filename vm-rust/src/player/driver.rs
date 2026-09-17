//! Canonical session-owned handler continuation.
//!
//! A continuation advances in synchronous turns. A turn borrows a player only
//! through `RuntimeSession::with_player` and returns before any host request is
//! queued or awaited. No pending action contains a VM borrow or a future that
//! executes against the ambient player.

use std::{collections::{HashMap, HashSet}, sync::Arc};
use manual_future::ManualFutureCompleter;
use log::warn;

use crate::director::lingo::{datum::{Datum, VarRef}, opcode::OpCode};

use super::{
    bytecode::{
        flow_control::{prepare_obj_call, PreparedObjCall},
        handler_manager::{try_execute_opcode_sync, BytecodeHandlerContext},
    },
    allocator::ScriptInstanceAllocatorTrait,
    cast_lib::CastMemberRef,
    debug::Breakpoint,
    datum_ref::DatumRef,
    ownership::OwnerToken,
    scope::ScopeResult,
    script::{set_obj_prop_sync, ScriptHandlerRef, SetObjPropOutcome},
    cast_lib::CastNotificationOutbox,
    session::{ExecutionContext, PlayerId, RuntimeSession},
    symbols::{builtin::BuiltInSymbol, symbol::Symbol},
    HandlerExecutionResult, ScriptError,
};

/// Session-wide checked completion identity. It is never reset when a
/// continuation finishes, so a late completion cannot target a later call.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ActionId(u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ActionKind {
    Breakpoint,
    ErrorPause,
    CooperativeYield,
    InternalInvocation,
    SetupCallback,
    Trace,
}

#[derive(Debug)]
struct ActionCapability;

/// The host sees only this opaque capability. Owner identity and scope token
/// remain in the session registry, where completion validation happens.
#[derive(Clone, Debug)]
pub(crate) struct CompletionTicket {
    id: ActionId,
    capability: Arc<ActionCapability>,
}

impl CompletionTicket {
    pub(crate) fn same_identity(&self, other: &Self) -> bool {
        self.id == other.id && Arc::ptr_eq(&self.capability, &other.capability)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ResumePhase {
    AfterBreakpoint,
    AfterStep,
    ApplyOpcode,
    ErrorUnwind,
    Teardown,
    SetupCallback,
}

#[derive(Clone)]
pub(crate) struct BreakpointRequest {
    pub(crate) ticket: CompletionTicket,
    pub(crate) breakpoint: Breakpoint,
    pub(crate) script_ref: CastMemberRef,
    pub(crate) handler_ref: ScriptHandlerRef,
    pub(crate) bytecode_index: usize,
}

#[derive(Clone)]
pub(crate) struct ErrorPauseRequest {
    pub(crate) ticket: CompletionTicket,
    pub(crate) error: ScriptError,
    pub(crate) script_ref: CastMemberRef,
    pub(crate) handler_ref: ScriptHandlerRef,
    pub(crate) bytecode_index: usize,
}

#[derive(Clone)]
pub(crate) struct CooperativeYieldRequest {
    pub(crate) ticket: CompletionTicket,
    pub(crate) deadline_ms: i64,
    pub(crate) backjumps: u32,
}

/// Internal VM calls retain VM handles because they are consumed by the owning
/// session. They are never sent to an external worker.
#[derive(Clone)]
pub(crate) enum InternalVmRequest {
    Construct {
        script: DatumRef,
        args: Vec<DatumRef>,
    },
    Global {
        name: Symbol,
        args: Vec<DatumRef>,
    },
    /// A global request resumed after an external Xtra probe declined the
    /// handler. It bypasses exactly one probe before ordinary dispatch.
    GlobalAfterExternalProbe {
        owner: OwnerToken,
        name: Symbol,
        args: Vec<DatumRef>,
    },
    Object {
        receiver: DatumRef,
        name: Symbol,
        args: Vec<DatumRef>,
    },
    /// An owner/generation-bound Flash host operation. The session executor
    /// must invoke it after releasing the player borrow and fence its decoded
    /// response before allocating any result datum.
    Flash(crate::player::handlers::datum_handlers::flash_object::FlashRequest),
    /// An object call which has already been classified as an asynchronous
    /// sprite operation.  The receiver and owned argument handles stay with
    /// the action until the host completes it; the action's owner/scope
    /// capability provides the player lifetime fence.
    SpriteAsync(SpriteAsyncRequest),
    /// A cast-member operation which must cross a host/network wait (W3D
    /// load/import or Havok callback execution).  It is deliberately a
    /// separate request kind so session execution never redispatches it as a
    /// synchronous object call.
    CastMemberAsync(CastMemberAsyncRequest),
    /// Movie work is prepared against one explicit player owner and executed
    /// by the host/session coordinator without redispatching ambient state.
    MovieAsync(super::handlers::movie::MovieAsyncRequest),
    ObjectV4 {
        receiver: DatumRef,
        name: Symbol,
        args: Vec<DatumRef>,
    },
    /// Property lookup retained by the async driver so a Flash receiver can
    /// cross the owner-bound host boundary without making every property type
    /// use the JS path.
    ObjectProperty {
        receiver: DatumRef,
        name: Symbol,
    },
    SetProperty {
        receiver: DatumRef,
        name: Symbol,
        value: DatumRef,
    },
    /// Owner-bound evaluation of a String/Chunk `.value` expression. The
    /// evaluator owns the source and returns through the existing EvalId pump.
    EvaluateValue {
        source: DatumRef,
        mode: ValueEvaluationMode,
    },
    /// A prepared external Xtra call executed after releasing the VM borrow.
    ExternalXtra(crate::player::xtra::external::ExternalXtraRequest),
    /// A pending owner-local Xtra load whose continuation remains in the VM.
    ExternalXtraLoad(crate::player::xtra::external::ExternalXtraLoadRequest),
    /// A built-in Multiuser/Curl operation prepared with owned transport data.
    /// The intent carries the player owner and instance generation fence.
    XtraPending(crate::player::xtra::manager::XtraPendingIntent),
    Tell {
        target: Option<NestedTargetIdentity>,
        name: Symbol,
        args: Vec<DatumRef>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ValueEvaluationMode {
    StringPropertyFallback,
    GlobalVoid,
}

/// Owned Flash action data.  `sprite_num` is copied before the request leaves
/// the context; the owner token rejects a reused player while the host waits.
#[derive(Clone)]
pub(crate) struct SpriteAsyncRequest {
    pub(crate) player_id: PlayerId,
    pub(crate) owner: OwnerToken,
    pub(crate) receiver: DatumRef,
    pub(crate) sprite_num: i16,
    pub(crate) handler: Symbol,
    pub(crate) args: Vec<DatumRef>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CastMemberAsyncKind {
    ImportFileInto,
    LoadFile,
    HavokStep,
}

/// Owned cast-member operation data. W3D arguments are prepared before the
/// request is emitted, so a later executor never re-reads VM handles to find
/// its URL or flags. Havok retains its receiver/args for callback sequencing.
#[derive(Clone)]
pub(crate) struct CastMemberAsyncRequest {
    pub(crate) player_id: PlayerId,
    pub(crate) owner: OwnerToken,
    pub(crate) receiver: DatumRef,
    pub(crate) member_ref: CastMemberRef,
    pub(crate) kind: CastMemberAsyncKind,
    pub(crate) handler: Symbol,
    pub(crate) args: Vec<DatumRef>,
    pub(crate) file_name: Option<String>,
    pub(crate) overwrite: bool,
    pub(crate) generate_unique_names: bool,
}

/// Owner-bound re-entrancy key for a frame/movie static event.  The event
/// arguments are part of the key because the same handler may legitimately be
/// nested with different arguments.
#[derive(Clone, Debug)]
pub(crate) struct StaticEventGuard {
    pub(crate) owner: OwnerToken,
    pub(crate) registration_id: u64,
    pub(crate) member_ref: CastMemberRef,
    pub(crate) handler_name: String,
    pub(crate) args: Vec<DatumRef>,
}

/// Evaluator requests use the same owned invocation envelope as handler
/// continuations. Keeping this conversion in the driver prevents callers from
/// smuggling borrowed VM state across a host wait.
pub(crate) fn eval_request_kind(request: &InternalVmRequest) -> ActionKind {
    match request {
        InternalVmRequest::Global { .. }
        | InternalVmRequest::GlobalAfterExternalProbe { .. } => ActionKind::InternalInvocation,
        InternalVmRequest::Object { .. }
        | InternalVmRequest::ObjectV4 { .. }
        | InternalVmRequest::ObjectProperty { .. }
        | InternalVmRequest::Flash(_) => ActionKind::InternalInvocation,
        InternalVmRequest::SpriteAsync(_)
        | InternalVmRequest::CastMemberAsync(_)
        | InternalVmRequest::MovieAsync(_)
        | InternalVmRequest::XtraPending(_) => ActionKind::InternalInvocation,
        InternalVmRequest::SetProperty { .. } => ActionKind::InternalInvocation,
        InternalVmRequest::EvaluateValue { .. } => ActionKind::InternalInvocation,
        InternalVmRequest::Construct { .. } | InternalVmRequest::Tell { .. } => ActionKind::InternalInvocation,
        InternalVmRequest::ExternalXtra(_) | InternalVmRequest::ExternalXtraLoad(_) => {
            ActionKind::InternalInvocation
        }
    }
}

/// Return the executor-facing operation name retained with a deferred typed
/// request.  The host sees the original request payload and this reason, so
/// it can select readiness, import, or physics work without redispatching the
/// object call through the synchronous datum dispatcher.
pub(crate) fn async_request_reason(request: &InternalVmRequest) -> Option<String> {
    match request {
        InternalVmRequest::SpriteAsync(request) => Some(format!(
            "sprite {} requires Flash readiness/action executor",
            request.sprite_num
        )),
        InternalVmRequest::CastMemberAsync(request) => match request.kind {
            CastMemberAsyncKind::LoadFile => Some(format!(
                "cast member loadFile({}) requires owner-bound executor (overwrite={}, unique={})",
                request.file_name.as_deref().unwrap_or_default(),
                request.overwrite,
                request.generate_unique_names,
            )),
            CastMemberAsyncKind::ImportFileInto => Some(
                "cast member importFileInto requires owner-bound executor".to_owned(),
            ),
            CastMemberAsyncKind::HavokStep => Some(
                "cast member Havok step requires owner-bound callback executor".to_owned(),
            ),
        },
        InternalVmRequest::MovieAsync(request) => Some(format!(
            "movie {:?} requires owner-bound executor for player {}",
            request.kind, request.player_id
        )),
        InternalVmRequest::Flash(request) => Some(format!(
            "Flash {} on sprite {} requires owner/generation-bound host execution",
            request.path, request.sprite_num
        )),
        InternalVmRequest::ExternalXtra(request) => Some(format!(
            "external Xtra '{}' requires owner-bound plugin execution",
            request.xtra_name
        )),
        InternalVmRequest::ExternalXtraLoad(request) => Some(format!(
            "external Xtra '{}' requires owner-bound load completion",
            request.name
        )),
        InternalVmRequest::XtraPending(intent) => Some(match intent {
            super::xtra::manager::XtraPendingIntent::MultiuserConnect { .. } =>
                "Multiuser connect requires owner-bound WebSocket executor".to_owned(),
            super::xtra::manager::XtraPendingIntent::MultiuserSend { .. } =>
                "Multiuser send requires owner-bound WebSocket executor".to_owned(),
            super::xtra::manager::XtraPendingIntent::CurlExec { .. } =>
                "Curl execAsync requires owner-bound fetch executor".to_owned(),
            super::xtra::manager::XtraPendingIntent::FileIoOpen(_) =>
                "FileIO openFile requires owner-bound network executor".to_owned(),
            super::xtra::manager::XtraPendingIntent::SysMenu(_) =>
                "SysMenu host effect requires owner-bound browser executor".to_owned(),
            super::xtra::manager::XtraPendingIntent::BudApi(_) =>
                "BudAPI host effect requires owner-bound browser executor".to_owned(),
            super::xtra::manager::XtraPendingIntent::OpenUrl(_) =>
                "OpenURL host effect requires owner-bound browser executor".to_owned(),
        }),
        _ => None,
    }
}

pub(crate) enum GlobalDispatch {
    Child {
        receiver: Option<super::ScriptInstanceRef>,
        handler_ref: ScriptHandlerRef,
    },
    ChildPrepared {
        receiver: Option<super::ScriptInstanceRef>,
        handler_ref: ScriptHandlerRef,
        args: Vec<DatumRef>,
    },
    ChildWithCompletion {
        receiver: Option<super::ScriptInstanceRef>,
        handler_ref: ScriptHandlerRef,
        args: Vec<DatumRef>,
        completion: ChildCompletion,
    },
    AncestorChildren {
        calls: Vec<super::handlers::types::AncestorCall>,
    },
    /// Ordered behavior dispatch used by sendSprite/sendAllSprites. The
    /// driver aggregates the last non-void child result while preserving the
    /// original receiver order.
    ChildSequence { plan: BroadcastPlan },
    SyncResult(Result<DatumRef, ScriptError>),
    /// A global form whose first argument was prepared as an owner-bound
    /// typed request. The caller must retain this request; redispatching the
    /// original global name would lose consumed receiver semantics.
    PendingRequest {
        request: InternalVmRequest,
        reason: String,
    },
    Pending { reason: String },
}

/// A static frame/movie script callback used only after all behavior receivers
/// declined a broadcast.  The member and handler are retained, but the
/// handler reference is deliberately resolved at execution time so a prior
/// callback can mutate the script table without stale dispatch.
#[derive(Clone, Debug)]
pub(crate) struct StaticEventCall {
    pub(crate) member_ref: CastMemberRef,
    pub(crate) receiver: Option<super::ScriptInstanceRef>,
    pub(crate) handler_name: Symbol,
    pub(crate) args: Vec<DatumRef>,
}

#[derive(Clone, Debug)]
pub(crate) struct BroadcastPlan {
    pub(crate) calls: Vec<super::handlers::types::AncestorCall>,
    pub(crate) fallback: Vec<StaticEventCall>,
    pub(crate) initial_return: DatumRef,
    pub(crate) handled: bool,
    pub(crate) continue_on_error: bool,
}

pub(crate) fn classify_async_object(
    runtime: &mut ExecutionContext<'_>,
    receiver: &DatumRef,
    name: &Symbol,
    args: &[DatumRef],
) -> Result<InternalVmRequest, ScriptError> {
    // Clone the shallow receiver tag before preparing any typed payload. This
    // releases the allocator borrow so load preparation can use the mutable
    // symbol table without overlapping an immutable VM borrow.
    let value = checked_internal_datum(runtime.player, runtime.symbols, receiver)?.clone();
    // The request owns every argument across the deferred turn. Reject stale
    // or foreign handles before queuing it, while leaving each handler's
    // ordinary conversion/fallback rules to its eventual executor.
    for arg in args {
        checked_internal_datum(runtime.player, runtime.symbols, arg)?;
    }
    let player_id = runtime.player_id;
    let owner = runtime.player.owner.clone();
    let load_data = match (&value, name.eq_builtin(BuiltInSymbol::LoadFile)) {
        (Datum::CastMember(member_ref), true) => Some(
            crate::player::handlers::datum_handlers::cast_member::shockwave3d::Shockwave3dMemberHandlers::prepare_load_file(
                runtime, member_ref, args,
            )?,
        ),
        _ => None,
    };
    let request = match value {
        Datum::FlashObjectRef(_) => {
            crate::player::handlers::datum_handlers::flash_object::FlashObjectDatumHandlers::prepare_call(
                runtime.player,
                runtime.symbols,
                receiver,
                name.clone(),
                args,
            )
            .map(InternalVmRequest::Flash)?
        }
        Datum::SpriteRef(sprite_num) if name.eq_builtin(BuiltInSymbol::GetVariable) => {
            let path = args
                .first()
                .ok_or_else(|| ScriptError::new("getVariable requires a path".to_owned()))
                .and_then(|arg| checked_internal_datum(runtime.player, runtime.symbols, arg))?
                .string_value(runtime.symbols)?;
            let return_as_object = args.get(1)
                .map(|arg| checked_internal_datum(runtime.player, runtime.symbols, arg)
                    .map(|datum| datum.int_value().unwrap_or(1) == 0))
                .transpose()?
                .unwrap_or(false);
            let (cast_lib, cast_member) = runtime
                .player
                .movie
                .score
                .get_sprite(sprite_num)
                .and_then(|sprite| sprite.member.as_ref())
                .map(|member| (member.cast_lib, member.cast_member))
                .unwrap_or((0, 0));
            InternalVmRequest::Flash(
                crate::player::handlers::datum_handlers::flash_object::FlashObjectDatumHandlers::prepare_bind_get(
                    runtime.player,
                    sprite_num,
                    crate::player::handlers::datum_handlers::sprite::root_flash_path(&path),
                    return_as_object,
                    cast_lib,
                    cast_member,
                )?,
            )
        }
        Datum::SpriteRef(sprite_num) => InternalVmRequest::SpriteAsync(SpriteAsyncRequest {
            player_id,
            owner,
            receiver: receiver.clone(),
            sprite_num,
            handler: name.clone(),
            args: args.to_vec(),
        }),
        Datum::CastMember(member_ref)
            if name.eq_builtin(BuiltInSymbol::ImportFileInto)
                || name.eq_builtin(BuiltInSymbol::LoadFile)
                || (name.eq_builtin(BuiltInSymbol::Step)
                    && runtime
                        .player
                        .movie
                        .cast_manager
                        .find_member_by_ref(&member_ref)
                        .is_some_and(|member| {
                            matches!(
                                member.member_type,
                                crate::player::cast_member::CastMemberType::HavokPhysics(_)
                            )
                        })) =>
        {
            InternalVmRequest::CastMemberAsync(CastMemberAsyncRequest {
                player_id,
                owner,
                receiver: receiver.clone(),
                member_ref: member_ref.clone(),
                kind: if name.eq_builtin(BuiltInSymbol::ImportFileInto) {
                    CastMemberAsyncKind::ImportFileInto
                } else if name.eq_builtin(BuiltInSymbol::LoadFile) {
                    CastMemberAsyncKind::LoadFile
                } else {
                    CastMemberAsyncKind::HavokStep
                },
                handler: name.clone(),
                args: args.to_vec(),
                file_name: load_data.as_ref().map(|(file_name, _, _)| file_name.clone()),
                overwrite: load_data.as_ref().map_or(true, |(_, overwrite, _)| *overwrite),
                generate_unique_names: load_data
                    .as_ref()
                    .map_or(true, |(_, _, unique)| *unique),
            })
        }
        _ => InternalVmRequest::Object {
            receiver: receiver.clone(),
            name: name.clone(),
            args: args.to_vec(),
        },
    };
    Ok(request)
}

#[derive(Clone, Debug)]
pub(crate) enum ChildCompletion {
    ConstructScript { fallback: DatumRef },
    TimeoutNew { timeout_name: String, fallback: DatumRef },
    TimeoutForget { timeout_name: String },
}

#[derive(Clone, Debug)]
pub(crate) struct NestedTargetIdentity {
    pub(crate) player_id: PlayerId,
    pub(crate) owner: OwnerToken,
}

#[derive(Clone, Debug)]
pub(crate) enum InternalResultEffect {
    Construct,
    Global {
        push_return: bool,
        stop_current_handler: bool,
        return_value: Option<DatumRef>,
    },
    ObjectV4 {
        push_return: bool,
        route_to_global: bool,
    },
    SetProperty,
    Tell {
        push_return: bool,
        target: Option<NestedTargetIdentity>,
        is_return: bool,
        return_value: Option<DatumRef>,
    },
}

#[derive(Clone)]
pub(crate) struct InternalInvocationRequest {
    pub(crate) ticket: CompletionTicket,
    pub(crate) scope: super::ScopeToken,
    pub(crate) request: InternalVmRequest,
    /// Why an internal global call remains owned by the external execution
    /// loop. This is retained with the serialized request so an async global
    /// cannot accidentally fall through into a synchronous handler.
    pub(crate) pending_reason: Option<String>,
    pub(crate) effect: Option<InternalResultEffect>,
}

/// Host work contains serialized values, URLs, bytes, and primitives only.
/// `CompletionTicket` is opaque and cannot be forged outside this module.
#[derive(Clone)]
pub(crate) enum HostRequest {
    Fetch {
        ticket: CompletionTicket,
        url: String,
        request_bytes: Vec<u8>,
    },
    JavaScript {
        ticket: CompletionTicket,
        script: String,
        args_bytes: Vec<u8>,
    },
    Debugger {
        ticket: CompletionTicket,
        message: String,
    },
}

pub(crate) struct SetupCallbackPlan {
    pub(crate) player_id: PlayerId,
    pub(crate) owner: OwnerToken,
    pub(crate) kind: SetupCallbackKind,
    pub(crate) script_ref: CastMemberRef,
    pub(crate) handler_name: String,
    pub(crate) receiver: Option<Datum>,
    pub(crate) args: Vec<Datum>,
    pub(crate) retained_plan: Option<super::HandlerPlan>,
    pub(crate) expectation: super::SetupExpectation,
    pub(crate) child_policy: Option<ChildReturnPolicy>,
    pub(crate) push_return: bool,
}

#[derive(Clone)]
pub(crate) struct SetupCallbackRequest {
    pub(crate) ticket: CompletionTicket,
    pub(crate) player_id: PlayerId,
    pub(crate) owner: OwnerToken,
    pub(crate) kind: SetupCallbackKind,
    pub(crate) script_ref: CastMemberRef,
    pub(crate) handler_name: String,
    pub(crate) receiver: Option<Datum>,
    pub(crate) args: Vec<Datum>,
    pub(crate) retained_plan: Option<super::HandlerPlan>,
    pub(crate) expectation: super::SetupExpectation,
    pub(crate) child_policy: Option<ChildReturnPolicy>,
    pub(crate) push_return: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SetupCallbackKind {
    Virtual,
    JavaScript,
    Trace,
}

#[derive(Clone)]
pub(crate) struct TraceRequest {
    pub(crate) ticket: CompletionTicket,
    pub(crate) message: String,
}

#[derive(Clone)]
pub(crate) enum PendingAction {
    Breakpoint(BreakpointRequest),
    ErrorPause(ErrorPauseRequest),
    CooperativeYield(CooperativeYieldRequest),
    Internal(InternalInvocationRequest),
    CastLoad {
        ticket: CompletionTicket,
        request: super::cast_lib::CastLoadRequest,
    },
    Host(HostRequest),
    Setup(SetupCallbackRequest),
    Trace(TraceRequest),
}

impl PendingAction {
    /// Return the capability carried by this action, including host and cast work.
    pub(crate) fn ticket(&self) -> &CompletionTicket {
        match self {
            Self::Breakpoint(request) => &request.ticket,
            Self::ErrorPause(request) => &request.ticket,
            Self::CooperativeYield(request) => &request.ticket,
            Self::Internal(request) => &request.ticket,
            Self::CastLoad { ticket, .. } => ticket,
            Self::Host(HostRequest::Fetch { ticket, .. }
                | HostRequest::JavaScript { ticket, .. }
                | HostRequest::Debugger { ticket, .. }) => ticket,
            Self::Setup(request) => &request.ticket,
            Self::Trace(request) => &request.ticket,
        }
    }
}


/// Owner-bound command/evaluator work retained by the frontend pump. The
/// optional evaluator fields route a child completion back through the real
/// EvalId/EvalAction capability instead of the primary driver.
pub(crate) struct PendingCommand {
    pub(crate) player_id: PlayerId,
    pub(crate) owner: OwnerToken,
    pub(crate) action: Option<PendingAction>,
    /// `false` means the action has just been enqueued and is ready for the
    /// owner scheduler. A retained external wait is `true`, so it is never
    /// redispatched until its exact completion arrives.
    pub(crate) started: bool,
    pub(crate) ticket: Option<CompletionTicket>,
    pub(crate) completer: Option<ManualFutureCompleter<Result<DatumRef, ScriptError>>>,
    pub(crate) event_sender: Option<async_std::channel::Sender<Result<ScopeResult, ScriptError>>>,
    pub(crate) score_continuation: Option<crate::player::score::BehaviorDefaultsContinuation>,
    pub(crate) child_completion: Option<ChildCompletion>,
    pub(crate) eval_child: Option<crate::player::eval::EvalId>,
    pub(crate) eval_sender: Option<async_std::channel::Sender<Result<DatumRef, ScriptError>>>,
}


pub(crate) enum DriverPhase {
    Setup(SetupStage),
    Running,
    Awaiting {
        ticket: CompletionTicket,
        resume: ResumePhase,
    },
    /// A host completion was accepted and is retained until the next driver
    /// increment applies its payload at the recorded resume phase. Keeping the
    /// completion here prevents a successful acknowledgement from silently
    /// dropping an owned VM value or re-running the original opcode.
    Resuming {
        resume: ResumePhase,
        completion: ActionCompletion,
    },
    Completing,
    Failed(ScriptError),
    Cancelled,
}

/// Setup progression is explicit so a callback completion cannot repeat a
/// virtual call, JS call, scope push, trace, or IR compilation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SetupStage {
    Virtual,
    JavaScript,
    ScopePush,
    Trace,
    IrCompile,
}

pub(crate) enum DriverStart {
    Early(ScopeResult),
    Running(DriverContinuation),
}

pub(crate) enum DriverTurn {
    Waiting,
    Pending(PendingAction),
    Complete(ScopeResult),
    Error(ScriptError),
}

#[derive(Clone, Copy, Debug)]
enum ChildDelivery {
    GlobalScope,
    ObjCallV4,
}

#[derive(Clone, Debug)]
pub(crate) enum ChildReturnPolicy {
    GlobalScope { ext_call_layer: bool },
    ObjCallV4,
    Ancestor {
        remaining: Vec<super::handlers::types::AncestorCall>,
        final_push_return: bool,
        ext_call_layer: bool,
        delivery: Option<ChildDelivery>,
    },
    Broadcast {
        remaining: Vec<super::handlers::types::AncestorCall>,
        fallback: Vec<StaticEventCall>,
        continue_on_error: bool,
        final_push_return: bool,
        ext_call_layer: bool,
        delivery: Option<ChildDelivery>,
        last_return_value: DatumRef,
        handled: bool,
    },
    Completion {
        completion: ChildCompletion,
        ext_call_layer: bool,
        delivery: Option<ChildDelivery>,
    },
}

/// The frame stack and exact resume phase belong to this object. It replaces
/// the old split `ctx`/`ir_state`/`parents` locals in `mod.rs`.
pub(crate) struct DriverContinuation {
    pub(crate) player_id: PlayerId,
    /// Owner identity captured with the frame stack. Numeric player IDs can be
    /// reused after reset/removal, so terminal cleanup must retain this token.
    pub(crate) owner: OwnerToken,
    pub(crate) phase: DriverPhase,
    pub(crate) frames: Vec<super::HandlerFrame>,
    pub(crate) backjumps: u32,
    pub(crate) total_backjumps: u32,
    pub(crate) last_yield_ms: i64,
    /// Return-push state for the one session-owned ObjCall pending path.
    /// Other internal resume states retain their existing behavior until their
    /// own payload application is migrated.
    obj_call_push_return: Option<bool>,
    /// Return policy for each frame. Global ExtCall children update their
    /// caller scope's return value; ordinary ObjCall children only deliver
    /// the optional stack result.
    child_return_policies: Vec<Option<ChildReturnPolicy>>,
    /// Completion effects for the opcode-preparation path. This is separate
    /// from the established ObjCall field so its resume policy cannot alter
    /// the already-migrated ObjCall behavior.
    internal_effect: Option<InternalResultEffect>,
    /// Legacy ExtCall accounts for one handler stack layer around the whole
    /// invocation. Real child frames add their own layers through
    /// `setup_handler_frame`; this count tracks only the extra ExtCall layer.
    ext_call_layers: Vec<super::ScopeToken>,
    /// Whether the currently pending internal action owns an ExtCall layer.
    /// The layer is released when its accepted completion is applied.
    internal_ext_call_layer: bool,
    static_event_guard: Option<StaticEventGuard>,
    /// Keep Havok's force-routing mode active for every synchronous turn of a
    /// callback child, including turns after a host suspension. The wrapper
    /// restores the prior thread-local value before returning to the host.
    pub(crate) step_callback: bool,
    /// Setup plans are retained until the external JS executor returns. They
    /// are prepared before the first scope push and never hold VM borrows.
    setup_pending: Option<SetupCallbackRequest>,
    setup_child_policy: Option<ChildReturnPolicy>,
    setup_parent_scope: Option<super::ScopeToken>,
    setup_push_return: bool,
    setup_expectation: Option<super::SetupExpectation>,
}

fn prepare_chained_owner_request(
    runtime: &mut super::session::ExecutionContext<'_>,
    frame_ctx: &BytecodeHandlerContext,
) -> Result<Option<InternalVmRequest>, ScriptError> {
    if !frame_ctx.scope.validate_top(runtime.player) {
        return Err(super::cancelled_scope_error());
    }
    let bytecode = runtime.player.get_ctx_current_bytecode(frame_ctx);
    let property = checked_context_name(frame_ctx, bytecode.obj as u16)?;
    let receiver = {
        let scope = runtime
            .player
            .scopes
            .get_mut(frame_ctx.scope.slot())
            .ok_or_else(super::cancelled_scope_error)?;
        scope
            .stack
            .last_ref_with(&mut runtime.player.allocator, &mut runtime.player.bitmap_manager)
            .unwrap_or(DatumRef::Void)
    };
    let receiver_type = checked_internal_datum(runtime.player, runtime.symbols, &receiver)?.type_enum();
    let request_kind = if matches!(
        receiver_type,
        crate::director::lingo::datum::DatumType::JsObjectRef
    ) {
        Some(None)
    } else if matches!(
        receiver_type,
        crate::director::lingo::datum::DatumType::String
            | crate::director::lingo::datum::DatumType::StringChunk
    ) && property == Symbol::builtin(BuiltInSymbol::Value)
    {
        Some(Some(ValueEvaluationMode::StringPropertyFallback))
    } else {
        None
    };
    let Some(mode) = request_kind else {
        return Ok(None);
    };
    let receiver = {
        let scope = runtime
            .player
            .scopes
            .get_mut(frame_ctx.scope.slot())
            .ok_or_else(super::cancelled_scope_error)?;
        scope
            .stack
            .pop_ref_with(&mut runtime.player.allocator, &mut runtime.player.bitmap_manager)
            .unwrap_or(DatumRef::Void)
    };
    Ok(Some(if let Some(mode) = mode {
        InternalVmRequest::EvaluateValue {
            source: receiver,
            mode,
        }
    } else {
        InternalVmRequest::ObjectProperty {
            receiver,
            name: property,
        }
    }))
}

impl DriverContinuation {
    fn release_static_event_guard(&mut self, session: &mut RuntimeSession) {
        if let Some(guard) = self.static_event_guard.take() {
            let _ = session.release_static_event_guard(self.player_id, &guard);
        }
    }

    fn policy_has_ext_call_layer(policy: Option<&ChildReturnPolicy>) -> bool {
        matches!(
            policy,
            Some(ChildReturnPolicy::GlobalScope { ext_call_layer: true })
                | Some(ChildReturnPolicy::Ancestor { ext_call_layer: true, .. })
                | Some(ChildReturnPolicy::Broadcast { ext_call_layer: true, .. })
                | Some(ChildReturnPolicy::Completion { ext_call_layer: true, .. })
        )
    }

    fn policy_is_global_scope(policy: Option<&ChildReturnPolicy>) -> bool {
        matches!(
            policy,
            Some(ChildReturnPolicy::GlobalScope { .. })
                | Some(ChildReturnPolicy::Ancestor { delivery: Some(ChildDelivery::GlobalScope), .. })
                | Some(ChildReturnPolicy::Broadcast { delivery: Some(ChildDelivery::GlobalScope), .. })
                | Some(ChildReturnPolicy::Completion { delivery: Some(ChildDelivery::GlobalScope), .. })
        )
    }

    fn policy_is_obj_call(policy: Option<&ChildReturnPolicy>) -> bool {
        matches!(
            policy,
            Some(ChildReturnPolicy::ObjCallV4)
                | Some(ChildReturnPolicy::Ancestor { delivery: Some(ChildDelivery::ObjCallV4), .. })
                | Some(ChildReturnPolicy::Broadcast { delivery: Some(ChildDelivery::ObjCallV4), .. })
                | Some(ChildReturnPolicy::Completion { delivery: Some(ChildDelivery::ObjCallV4), .. })
        )
    }

    /// Completion delivery is independent of whether the invocation also owns
    /// an ExtCall layer.  In particular, an ObjectV4 routed through the global
    /// dispatcher can have `ext_call_layer == false` while still needing the
    /// caller's `last_handler_result` update.
    fn policy_updates_last_handler_result(policy: Option<&ChildReturnPolicy>) -> bool {
        Self::policy_is_global_scope(policy) || Self::policy_is_obj_call(policy)
    }

    fn resolve_ancestor_pending(
        &mut self,
        session: &mut RuntimeSession,
        call: &super::handlers::types::AncestorCall,
    ) -> Result<Option<super::PendingCall>, ScriptError> {
        Self::with_context(session, self.player_id, |runtime| {
            super::handlers::types::resolve_ancestor_call(runtime, call)
        })
        .ok_or_else(super::cancelled_scope_error)
        .and_then(|resolved| resolved)
        .map(|resolved| {
            resolved.map(|(receiver, handler_ref, args)| super::PendingCall {
                receiver: Some(receiver),
                handler_ref,
                args,
                use_raw_arg_list: true,
                push_return: false,
            })
        })
    }

    fn resolve_broadcast_pending(
        &mut self,
        session: &mut RuntimeSession,
        call: &super::handlers::types::AncestorCall,
    ) -> Result<Option<super::PendingCall>, ScriptError> {
        let pending = self.resolve_ancestor_pending(session, call)?;
        Ok(pending.map(|mut pending| {
            // Broadcast descriptors retain user arguments.  The child setup
            // adds its receiver (`me`) exactly once; callAncestor descriptors
            // remain on the raw path above because they already include it.
            pending.use_raw_arg_list = false;
            pending
        }))
    }

    fn resolve_static_pending(
        &mut self,
        session: &mut RuntimeSession,
        call: &StaticEventCall,
    ) -> Result<Option<super::PendingCall>, ScriptError> {
        let resolved = Self::with_context(session, self.player_id, |runtime| {
            super::compare::validate_direct_symbol_fields(
                &Datum::Symbol(call.handler_name.clone()),
                runtime.symbols,
            )?;
            if let Some(receiver) = &call.receiver {
                runtime
                    .player
                    .allocator
                    .get_script_instance_opt(receiver)
                    .ok_or_else(|| ScriptError::new_code(
                        super::ScriptErrorCode::InvalidReference,
                        "stale static event receiver".to_owned(),
                    ))?;
            }
            let Some(script) = runtime.player.movie.cast_manager.get_script_by_ref(&call.member_ref)
            else {
                return Err(ScriptError::new_code(
                    super::ScriptErrorCode::InvalidReference,
                    format!("static event references missing script {:?}", call.member_ref),
                ));
            };
            let Some(handler_ref) = script.get_own_handler_ref(call.handler_name.clone()) else {
                return Ok(None);
            };
            Ok(Some((handler_ref, call.receiver.clone(), call.args.clone())))
        })
        .ok_or_else(super::cancelled_scope_error)
        .and_then(|resolved| resolved)?;
        let Some((handler_ref, receiver, args)) = resolved else {
            return Ok(None);
        };
        let Some(guard) = session.enter_static_event_guard(self.player_id, call)? else {
            return Ok(None);
        };
        self.static_event_guard = Some(guard);
        Ok(Some(super::PendingCall {
                receiver,
                handler_ref,
                args,
                use_raw_arg_list: true,
                push_return: false,
            }))
    }

    pub(crate) fn start(
        session: &mut RuntimeSession,
        player_id: PlayerId,
        receiver: Option<super::ScriptInstanceRef>,
        handler_ref: ScriptHandlerRef,
        args: &[DatumRef],
        use_raw_arg_list: bool,
    ) -> Result<DriverStart, ScriptError> {
        Self::start_subordinate(session, player_id, receiver, handler_ref, args, use_raw_arg_list, None)
    }

    /// Start a child continuation without claiming the player's primary
    /// driver slot. This is used by debugger/evaluator calls while a parent
    /// handler remains paused. The parent scope is captured as an active
    /// anchor and is checked again after setup and on every child turn.
    pub(crate) fn start_subordinate(
        session: &mut RuntimeSession,
        player_id: PlayerId,
        receiver: Option<super::ScriptInstanceRef>,
        handler_ref: ScriptHandlerRef,
        args: &[DatumRef],
        use_raw_arg_list: bool,
        parent_scope: Option<&super::ScopeToken>,
    ) -> Result<DriverStart, ScriptError> {
        let (owner, expectation) = Self::with_context(session, player_id, |ctx| {
            (
                ctx.player.owner.clone(),
                super::SetupExpectation::capture(ctx.player, parent_scope),
            )
        })
        .ok_or_else(super::cancelled_scope_error)?;
        match super::setup_handler_frame(
            session,
            player_id,
            receiver,
            handler_ref,
            &args.to_vec(),
            use_raw_arg_list,
            false,
            expectation,
        )? {
            super::FrameSetup::Early(result) => Ok(DriverStart::Early(result)),
            super::FrameSetup::Pending(plan) => {
                let ticket = session
                    .allocate_setup_action(&owner, plan.expectation.parent_scope().as_ref())
                    .ok_or_else(|| ScriptError::new("setup action ticket exhausted".to_owned()))?;
                let parent_scope = plan.expectation.parent_scope();
                let setup_expectation = plan.expectation.clone();
                let request = SetupCallbackRequest {
                    ticket,
                    player_id: plan.player_id,
                    owner: plan.owner,
                    kind: plan.kind,
                    script_ref: plan.script_ref,
                    handler_name: plan.handler_name,
                    receiver: plan.receiver,
                    args: plan.args,
                    retained_plan: plan.retained_plan,
                    expectation: plan.expectation,
                    child_policy: plan.child_policy,
                    push_return: plan.push_return,
                };
                Ok(DriverStart::Running(Self {
                    player_id,
                    owner,
                    phase: DriverPhase::Setup(SetupStage::JavaScript),
                    frames: vec![],
                    backjumps: 0,
                    total_backjumps: 0,
                    last_yield_ms: 0,
                    obj_call_push_return: None,
                    child_return_policies: vec![],
                    internal_effect: None,
                    ext_call_layers: Vec::new(),
                    internal_ext_call_layer: false,
                    static_event_guard: None,
                    step_callback: false,
                    setup_pending: Some(request),
                    setup_child_policy: None,
                    setup_parent_scope: parent_scope,
                    setup_push_return: plan.push_return,
                    setup_expectation: Some(setup_expectation),
                }))
            }
            super::FrameSetup::Frame(frame) => Ok(DriverStart::Running(Self {
                player_id,
                owner,
                phase: DriverPhase::Running,
                frames: vec![frame],
                backjumps: 0,
                total_backjumps: 0,
                last_yield_ms: 0,
                obj_call_push_return: None,
                child_return_policies: vec![None],
                internal_effect: None,
                ext_call_layers: Vec::new(),
                internal_ext_call_layer: false,
                static_event_guard: None,
                step_callback: false,
                setup_pending: None,
                setup_child_policy: None,
                setup_parent_scope: None,
                setup_push_return: false,
                setup_expectation: None,
            })),
        }
    }

    /// The only dispatcher-side player borrow. This closure must return before
    /// the scheduler can expose any action to a host or external worker.
    pub(crate) fn with_context<R>(
        session: &mut RuntimeSession,
        player_id: PlayerId,
        f: impl FnOnce(&mut ExecutionContext<'_>) -> R,
    ) -> Option<R> {
        session.with_player(player_id, |mut context| f(&mut context))
    }

    fn current_context(&self) -> Option<BytecodeHandlerContext> {
        self.frames.last().map(|frame| frame.ctx.clone())
    }

    fn current_scope_active(&self, session: &mut RuntimeSession) -> bool {
        let Some(frame) = self.frames.last() else {
            return false;
        };
        Self::with_context(session, self.player_id, |runtime| {
            frame.ctx.scope.validate_active(runtime.player)
        })
        .unwrap_or(false)
    }

    /// Retire every still-live frame, validating each token before mutating
    /// the owner arena. A stale current token stops the unwind without touching
    /// a replacement player or scope slot.
    pub(crate) fn cancel(&mut self, session: &mut RuntimeSession) -> bool {
        let mut frames = std::mem::take(&mut self.frames);
        self.release_static_event_guard(session);
        // Extra legacy layers belong to their parent scope. Release them while
        // that scope is still active, before unwinding any frame that may pop
        // it; token validation prevents touching a reset/replacement scope.
        self.release_all_ext_call_layers(session);
        self.child_return_policies.clear();
        let Some(current) = frames.pop() else {
            self.phase = DriverPhase::Cancelled;
            return true;
        };
        let unwound = Self::with_context(session, self.player_id, |runtime| {
            super::unwind_handler_frames(
                runtime.player,
                &current.ctx.scope,
                current.is_frame_script,
                &mut frames,
            )
        })
        .unwrap_or(false);
        self.phase = DriverPhase::Cancelled;
        unwound
    }

    fn enter_ext_call_layer(
        &mut self,
        session: &mut RuntimeSession,
        scope: &super::ScopeToken,
    ) -> bool {
        let entered = Self::with_context(session, self.player_id, |runtime| {
            if !runtime.player.owner.same_identity(&self.owner)
                || !scope.validate_active(runtime.player)
            {
                return false;
            }
            let Some(depth) = runtime.player.handler_stack_depth.checked_add(1) else {
                return false;
            };
            runtime.player.handler_stack_depth = depth;
            true
        })
        .unwrap_or(false);
        if entered {
            self.ext_call_layers.push(scope.clone());
        }
        entered
    }

    fn release_ext_call_layer(&mut self, session: &mut RuntimeSession) {
        let Some(scope) = self.ext_call_layers.pop() else {
            return;
        };
        let _ = Self::with_context(session, self.player_id, |runtime| {
            if runtime.player.owner.same_identity(&self.owner)
                && scope.validate_active(runtime.player)
            {
                runtime.player.handler_stack_depth =
                    runtime.player.handler_stack_depth.saturating_sub(1);
            }
        });
    }

    fn release_all_ext_call_layers(&mut self, session: &mut RuntimeSession) {
        while !self.ext_call_layers.is_empty() {
            self.release_ext_call_layer(session);
        }
        self.internal_ext_call_layer = false;
    }

    pub(crate) fn turn(&mut self, session: &mut RuntimeSession) -> DriverTurn {
        let prior = if self.step_callback {
            Self::with_context(session, self.player_id, |runtime| {
                if !self.owner.same_identity(&runtime.player.owner)
                    || !self.owner.is_arena_live()
                {
                    return None;
                }
                let prior = runtime.player.in_havok_step_callback;
                runtime.player.in_havok_step_callback = true;
                Some(prior)
            })
            .flatten()
        } else {
            None
        };
        let turn = self.turn_impl(session);
        if let Some(prior) = prior {
            let _ = Self::with_context(session, self.player_id, |runtime| {
                if self.owner.same_identity(&runtime.player.owner)
                    && self.owner.is_arena_live()
                {
                    runtime.player.in_havok_step_callback = prior;
                }
            });
        }
        turn
    }

    fn turn_impl(&mut self, session: &mut RuntimeSession) -> DriverTurn {
        match &self.phase {
            DriverPhase::Awaiting { .. } => {
                let active = if self.frames.is_empty() {
                    Self::with_context(session, self.player_id, |ctx| {
                        self.owner.same_identity(&ctx.player.owner) && self.owner.is_arena_live()
                    }).unwrap_or(false)
                } else {
                    self.current_scope_active(session)
                };
                if !active {
                    self.release_static_event_guard(session);
                    self.release_all_ext_call_layers(session);
                    self.phase = DriverPhase::Cancelled;
                    return DriverTurn::Error(super::cancelled_scope_error());
                }
                return DriverTurn::Waiting;
            }
            DriverPhase::Resuming { resume, .. } => {
                if *resume == ResumePhase::SetupCallback {
                    return self.apply_setup_resume(session);
                }
                if self.internal_effect.is_some() {
                    return self.apply_internal_resume(session);
                }
                if self.obj_call_push_return.is_some() {
                    return self.apply_obj_call_resume(session);
                }
                if !self.current_scope_active(session) {
                    self.release_static_event_guard(session);
                    self.release_all_ext_call_layers(session);
                    self.phase = DriverPhase::Cancelled;
                    return DriverTurn::Error(super::cancelled_scope_error());
                }
                return DriverTurn::Waiting;
            }
            DriverPhase::Failed(error) => return DriverTurn::Error(error.clone()),
            DriverPhase::Cancelled => {
                return DriverTurn::Error(super::cancelled_scope_error())
            }
            DriverPhase::Setup(_) => {
                let Some(request) = self.setup_pending.take() else {
                    return DriverTurn::Error(ScriptError::new(
                        "driver setup was not completed before turn".to_owned(),
                    ));
                };
                let ticket = request.ticket.clone();
                self.phase = DriverPhase::Awaiting {
                    ticket,
                    resume: ResumePhase::SetupCallback,
                };
                return DriverTurn::Pending(PendingAction::Setup(request));
            }
            DriverPhase::Completing => {
                return DriverTurn::Error(ScriptError::new(
                    "driver turn requested after completion".to_owned(),
                ))
            }
            DriverPhase::Running => {}
        }
        let Some(frame_ctx) = self.current_context() else {
            self.phase = DriverPhase::Completing;
            return DriverTurn::Complete(ScopeResult {
                return_value: DatumRef::Void,
                passed: false,
            });
        };
        let token = frame_ctx.scope.clone();
        let decoded = Self::with_context(session, self.player_id, |runtime| {
            if !token.validate_top(runtime.player) {
                return Err(super::cancelled_scope_error());
            }
            let scope = runtime.player.scopes.get(token.slot()).ok_or_else(
                super::cancelled_scope_error,
            )?;
            let Some(bytecode) = frame_ctx.code.handler.bytecode_array.get(scope.bytecode_index)
            else {
                return Ok(None);
            };
            Ok(Some((bytecode.opcode, scope.bytecode_index)))
        });
        let Some(decoded) = decoded else {
            self.release_all_ext_call_layers(session);
            self.phase = DriverPhase::Cancelled;
            return DriverTurn::Error(super::cancelled_scope_error());
        };
        let (opcode, _) = match decoded {
            Ok(Some(value)) => value,
            Ok(None) => return self.finish_current_frame(session),
            Err(error) => return self.fail_current_frame(session, error),
        };

        if opcode == OpCode::ObjCall {
            return self.turn_obj_call(session, frame_ctx, token);
        }

        if opcode == OpCode::GetChainedProp {
            let pending = Self::with_context(session, self.player_id, |runtime| {
                prepare_chained_owner_request(runtime, &frame_ctx)
            });
            let Some(pending) = pending else {
                self.release_all_ext_call_layers(session);
                self.phase = DriverPhase::Cancelled;
                return DriverTurn::Error(super::cancelled_scope_error());
            };
            match pending {
                Ok(Some(request)) => {
                    return self.prepare_pending_obj_request(
                        session,
                        frame_ctx,
                        request,
                        "owner-bound chained property requires deferred execution".to_owned(),
                        true,
                    );
                }
                Ok(None) => {}
                Err(error) => return self.fail_current_frame(session, error),
            }
        }

        let result = Self::with_context(session, self.player_id, |runtime| {
            if !token.validate_top(runtime.player) {
                return Some(Err(super::cancelled_scope_error()));
            }
            try_execute_opcode_sync(opcode, runtime, &frame_ctx)
        });
        let Some(result) = result else {
            self.release_all_ext_call_layers(session);
            self.phase = DriverPhase::Cancelled;
            return DriverTurn::Error(super::cancelled_scope_error());
        };
        let Some(result) = result else {
            return self.prepare_internal_action(session, opcode, frame_ctx);
        };
        match result {
            Ok(HandlerExecutionResult::Advance) => {
                self.advance_current_opcode(session, &frame_ctx, &token)
            }
            Ok(HandlerExecutionResult::Stop) => self.finish_current_frame(session),
            Ok(HandlerExecutionResult::Jump) => {
                self.total_backjumps = self.total_backjumps.saturating_add(1);
                DriverTurn::Waiting
            }
            Ok(HandlerExecutionResult::Call(pending)) => {
                self.start_child(session, pending)
            }
            Ok(HandlerExecutionResult::Error(error)) | Err(error) => {
                self.fail_current_frame(session, error)
            }
        }
    }

    fn turn_obj_call(
        &mut self,
        session: &mut RuntimeSession,
        frame_ctx: BytecodeHandlerContext,
        token: super::ScopeToken,
    ) -> DriverTurn {
        let prepared = Self::with_context(session, self.player_id, |runtime| {
            if !token.validate_top(runtime.player) {
                return Err(super::cancelled_scope_error());
            }
            prepare_obj_call(runtime, &frame_ctx)
        });
        let Some(prepared) = prepared else {
            self.phase = DriverPhase::Cancelled;
            return DriverTurn::Error(super::cancelled_scope_error());
        };
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(error) => return self.fail_current_frame(session, error),
        };

        if let Some((instance_ref, handler_ref)) = prepared.lingo_target {
            return self.start_child(
                session,
                super::PendingCall {
                    receiver: Some(instance_ref),
                    handler_ref,
                    args: prepared.args,
                    use_raw_arg_list: false,
                    push_return: prepared.push_return,
                },
            );
        }

        let dispatch = Self::with_context(session, self.player_id, |runtime| {
            if !token.validate_top(runtime.player) {
                return super::handlers::datum_handlers::SyncDatumCall::Handled(Err(
                    super::cancelled_scope_error(),
                ));
            }
            super::handlers::datum_handlers::try_call_datum_handler_sync(
                runtime,
                &prepared.receiver,
                prepared.name.clone(),
                &prepared.args,
            )
        });
        let Some(dispatch) = dispatch else {
            self.phase = DriverPhase::Cancelled;
            return DriverTurn::Error(super::cancelled_scope_error());
        };
        match dispatch {
            super::handlers::datum_handlers::SyncDatumCall::Child {
                receiver,
                handler_ref,
                args,
            } => self.start_child(
                session,
                super::PendingCall {
                    receiver,
                    handler_ref,
                    args,
                    // The prepared object arguments exclude `me`; setup must
                    // prepend the owned instance receiver for this handler.
                    use_raw_arg_list: false,
                    push_return: prepared.push_return,
                },
            ),
            super::handlers::datum_handlers::SyncDatumCall::ChildWithCompletion {
                receiver,
                handler_ref,
                args,
                completion,
            } => self.start_child_with_policy(
                session,
                super::PendingCall {
                    receiver,
                    handler_ref,
                    args,
                    use_raw_arg_list: false,
                    push_return: prepared.push_return,
                },
                Some(ChildReturnPolicy::Completion {
                    completion,
                    ext_call_layer: false,
                    delivery: None,
                }),
            ),
            super::handlers::datum_handlers::SyncDatumCall::Handled(Ok(result)) => {
                self.apply_obj_call_result(
                    session,
                    &frame_ctx,
                    &token,
                    result,
                    prepared.push_return,
                )
            }
            super::handlers::datum_handlers::SyncDatumCall::Handled(Err(error)) => {
                self.fail_current_frame(session, error)
            }
            super::handlers::datum_handlers::SyncDatumCall::Pending { request, reason } => {
                self.prepare_pending_obj_request(
                    session,
                    frame_ctx,
                    request,
                    reason,
                    prepared.push_return,
                )
            }
            super::handlers::datum_handlers::SyncDatumCall::Unsupported => {
                self.prepare_obj_call_action(session, frame_ctx, prepared)
            }
        }
    }

    fn advance_current_opcode(
        &mut self,
        session: &mut RuntimeSession,
        frame_ctx: &BytecodeHandlerContext,
        token: &super::ScopeToken,
    ) -> DriverTurn {
        let advanced = Self::with_context(session, self.player_id, |runtime| {
            if !token.validate_top(runtime.player) {
                return None;
            }
            let Some(scope) = runtime.player.scopes.get_mut(token.slot()) else {
                return None;
            };
            let final_opcode = scope.bytecode_index.saturating_add(1)
                >= frame_ctx.code.handler.bytecode_array.len();
            if final_opcode {
                Some(true)
            } else {
                scope.bytecode_index = scope.bytecode_index.saturating_add(1);
                Some(scope.stop_requested)
            }
        });
        let Some(advanced) = advanced.flatten() else {
            self.phase = DriverPhase::Cancelled;
            return DriverTurn::Error(super::cancelled_scope_error());
        };
        if advanced {
            self.finish_current_frame(session)
        } else {
            DriverTurn::Waiting
        }
    }

    fn apply_obj_call_result(
        &mut self,
        session: &mut RuntimeSession,
        frame_ctx: &BytecodeHandlerContext,
        token: &super::ScopeToken,
        result: DatumRef,
        push_return: bool,
    ) -> DriverTurn {
        let checked = Self::with_context(session, self.player_id, |runtime| {
            if !token.validate_top(runtime.player) {
                return Err(super::cancelled_scope_error());
            }
            validate_obj_call_result(runtime, &result)?;
            runtime.player.last_handler_result = result.clone();
            if push_return {
                let Some(scope) = runtime.player.scopes.get_mut(token.slot()) else {
                    return Err(super::cancelled_scope_error());
                };
                scope.stack.push(result.clone());
            }
            if !token.validate_top(runtime.player) {
                return Err(super::cancelled_scope_error());
            }
            Ok(())
        });
        match checked {
            Some(Ok(())) => self.advance_current_opcode(session, frame_ctx, token),
            Some(Err(error)) => self.fail_current_frame(session, error),
            None => {
                self.phase = DriverPhase::Cancelled;
                DriverTurn::Error(super::cancelled_scope_error())
            }
        }
    }

    fn prepare_pending_obj_request(
        &mut self,
        session: &mut RuntimeSession,
        frame_ctx: BytecodeHandlerContext,
        request: InternalVmRequest,
        reason: String,
        push_return: bool,
    ) -> DriverTurn {
        let Some(owner) = Self::with_context(session, self.player_id, |ctx| {
            frame_ctx.scope.validate_top(ctx.player).then(|| ctx.player.owner.clone())
        }).flatten() else {
            self.phase = DriverPhase::Cancelled;
            return DriverTurn::Error(super::cancelled_scope_error());
        };
        let Some(ticket) = session.allocate_action(
            &owner,
            &frame_ctx.scope,
            ActionKind::InternalInvocation,
            ResumePhase::ApplyOpcode,
        ) else {
            let error = ScriptError::new("driver action sequence exhausted".to_owned());
            self.phase = DriverPhase::Failed(error.clone());
            return DriverTurn::Error(error);
        };
        self.obj_call_push_return = Some(push_return);
        self.phase = DriverPhase::Awaiting {
            ticket: ticket.clone(),
            resume: ResumePhase::ApplyOpcode,
        };
        DriverTurn::Pending(PendingAction::Internal(InternalInvocationRequest {
            ticket,
            scope: frame_ctx.scope.clone(),
            request,
            pending_reason: Some(reason),
            effect: None,
        }))
    }

    fn prepare_obj_call_action(
        &mut self,
        session: &mut RuntimeSession,
        frame_ctx: BytecodeHandlerContext,
        prepared: PreparedObjCall,
    ) -> DriverTurn {
        let Some(owner) = Self::with_context(session, self.player_id, |ctx| {
            if !frame_ctx.scope.validate_top(ctx.player) {
                return None;
            }
            Some(ctx.player.owner.clone())
        }).flatten() else {
            self.phase = DriverPhase::Cancelled;
            return DriverTurn::Error(super::cancelled_scope_error());
        };
        let request = match Self::with_context(session, self.player_id, |runtime| {
            classify_async_object(
                runtime,
                &prepared.receiver,
                &prepared.name,
                &prepared.args,
            )
        }) {
            Some(Ok(request)) => request,
            Some(Err(error)) => return self.fail_current_frame(session, error),
            None => {
                self.phase = DriverPhase::Cancelled;
                return DriverTurn::Error(super::cancelled_scope_error());
            }
        };
        let Some(ticket) = session.allocate_action(
            &owner,
            &frame_ctx.scope,
            ActionKind::InternalInvocation,
            ResumePhase::ApplyOpcode,
        ) else {
            let error = ScriptError::new("driver action sequence exhausted".to_owned());
            self.phase = DriverPhase::Failed(error.clone());
            return DriverTurn::Error(error);
        };
        let action = PendingAction::Internal(InternalInvocationRequest {
            ticket: ticket.clone(),
            scope: frame_ctx.scope.clone(),
            request,
            pending_reason: None,
            effect: None,
        });
        self.obj_call_push_return = Some(prepared.push_return);
        self.phase = DriverPhase::Awaiting {
            ticket,
            resume: ResumePhase::ApplyOpcode,
        };
        DriverTurn::Pending(action)
    }

    fn apply_obj_call_resume(&mut self, session: &mut RuntimeSession) -> DriverTurn {
        let Some(frame_ctx) = self.current_context() else {
            self.release_all_ext_call_layers(session);
            self.phase = DriverPhase::Cancelled;
            return DriverTurn::Error(super::cancelled_scope_error());
        };
        let token = frame_ctx.scope.clone();
        let push_return = self.obj_call_push_return.take().unwrap_or(false);
        let completion = match &self.phase {
            DriverPhase::Resuming { completion, .. } => match completion {
                ActionCompletion::InternalResult(result) => Ok(result.clone()),
                ActionCompletion::InternalError(error) => Err(error.clone()),
                _ => Err(ScriptError::new("invalid ObjCall completion".to_owned())),
            },
            _ => Err(ScriptError::new("invalid ObjCall resume".to_owned())),
        };
        // The completion has already passed ticket/owner/scope authorization.
        // Leave the resumable state before applying the result so a successful
        // non-final ObjCall advances normally instead of re-entering Resuming.
        self.phase = DriverPhase::Running;
        match completion {
            Ok(result) => self.apply_obj_call_result(
                session,
                &frame_ctx,
                &token,
                result,
                push_return,
            ),
            Err(error) => self.fail_current_frame(session, error),
        }
    }

    fn apply_internal_resume(&mut self, session: &mut RuntimeSession) -> DriverTurn {
        let Some(frame_ctx) = self.current_context() else {
            self.release_all_ext_call_layers(session);
            self.phase = DriverPhase::Cancelled;
            return DriverTurn::Error(super::cancelled_scope_error());
        };
        let token = frame_ctx.scope.clone();
        let Some(effect) = self.internal_effect.take() else {
            self.release_all_ext_call_layers(session);
            self.phase = DriverPhase::Cancelled;
            return DriverTurn::Error(super::cancelled_scope_error());
        };
        let release_ext_call_layer = self.internal_ext_call_layer;
        self.internal_ext_call_layer = false;
        let retry_request = match &self.phase {
            DriverPhase::Resuming { completion, .. } => match completion {
                ActionCompletion::RetryInternal(request) => Some(request.clone()),
                _ => None,
            },
            _ => None,
        };
        if let Some(retry_request) = retry_request {
            self.phase = DriverPhase::Running;
            let (owner, name, args) = match retry_request.clone() {
                InternalVmRequest::GlobalAfterExternalProbe { owner, name, args } => {
                    (owner, name, args)
                }
                _ => {
                    if release_ext_call_layer {
                        self.release_ext_call_layer(session);
                    }
                    return self.fail_current_frame(
                        session,
                        ScriptError::new("invalid internal retry request".to_owned()),
                    );
                }
            };
            let owner_current = Self::with_context(session, self.player_id, |runtime| {
                owner.same_identity(&runtime.player.owner)
                    && owner.is_arena_live()
                    && runtime.player.owner.is_arena_live()
            })
            .unwrap_or(false);
            if !owner_current {
                if release_ext_call_layer {
                    self.release_ext_call_layer(session);
                }
                return self.fail_current_frame(session, super::cancelled_scope_error());
            }
            let dispatch = match session.dispatch_global_after_external_probe(self.player_id, &name, &args) {
                Ok(dispatch) => dispatch,
                Err(error) => {
                    if release_ext_call_layer {
                        self.release_ext_call_layer(session);
                    }
                    return self.fail_current_frame(session, error);
                }
            };
            return self.continue_global_dispatch(
                session,
                &frame_ctx,
                effect,
                release_ext_call_layer,
                retry_request,
                dispatch,
            );
        }
        let completion = match &self.phase {
            DriverPhase::Resuming { completion, .. } => match completion {
                ActionCompletion::InternalResult(result) => Ok(result.clone()),
                ActionCompletion::InternalError(error) => Err(error.clone()),
                _ => Err(ScriptError::new("invalid internal completion".to_owned())),
            },
            _ => Err(ScriptError::new("invalid internal resume".to_owned())),
        };
        self.phase = DriverPhase::Running;
        if release_ext_call_layer {
            self.release_ext_call_layer(session);
        }
        let completion_result = match completion {
            Ok(result) => result,
            Err(error) => return self.fail_current_frame(session, error),
        };
        let forced_result = match &effect {
            InternalResultEffect::Global {
                return_value: Some(return_value),
                ..
            }
            | InternalResultEffect::Tell {
                return_value: Some(return_value),
                ..
            } => Some(return_value.clone()),
            _ => None,
        };
        let result = forced_result.clone().unwrap_or(completion_result);

        if let InternalResultEffect::Tell { target: Some(target), .. } = &effect {
            let target_live = Self::with_context(session, target.player_id, |runtime| {
                runtime.player.owner.same_identity(&target.owner)
                    && runtime.player.owner.is_arena_live()
                    && target.owner.is_arena_live()
            })
            .unwrap_or(false);
            if !target_live {
                return self.fail_current_frame(
                    session,
                    ScriptError::new_code(
                        super::ScriptErrorCode::InvalidReference,
                        "tell target player was replaced or removed".to_owned(),
                    ),
                );
            }
        }

        self.apply_internal_result(session, &frame_ctx, &token, effect, result)
    }

    fn apply_internal_result(
        &mut self,
        session: &mut RuntimeSession,
        frame_ctx: &BytecodeHandlerContext,
        token: &super::ScopeToken,
        effect: InternalResultEffect,
        result: DatumRef,
    ) -> DriverTurn {
        let applied = Self::with_context(session, self.player_id, |runtime| {
            if !token.validate_top(runtime.player) {
                return Err(super::cancelled_scope_error());
            }
            let push_return = match &effect {
                InternalResultEffect::Construct => true,
                InternalResultEffect::Global { push_return, .. }
                | InternalResultEffect::ObjectV4 { push_return, .. }
                | InternalResultEffect::Tell { push_return, .. } => *push_return,
                InternalResultEffect::SetProperty => false,
            };
            let validate_result = internal_completion_requires_validation(&effect)
                || matches!(
                    &effect,
                    InternalResultEffect::Global {
                        return_value: Some(_),
                        ..
                    }
                        | InternalResultEffect::Tell {
                            return_value: Some(_),
                            ..
                        }
                );
            if validate_result {
                validate_obj_call_result(runtime, &result)?;
            }
            let (update_last, update_scope_return) = match &effect {
                InternalResultEffect::Global {
                    return_value: Some(_),
                    ..
                } => (false, true),
                InternalResultEffect::Global { .. } => (true, true),
                InternalResultEffect::ObjectV4 { .. } => (true, false),
                InternalResultEffect::Tell {
                    target: None,
                    return_value: Some(_),
                    ..
                } => (false, true),
                InternalResultEffect::Tell { target: None, .. } => (true, true),
                _ => (false, false),
            };
            if update_last {
                runtime.player.last_handler_result = result.clone();
            }
            if update_scope_return {
                let scope = runtime
                    .player
                    .scopes
                    .get_mut(token.slot())
                    .ok_or_else(super::cancelled_scope_error)?;
                scope.return_value = result.clone();
            }
            if push_return {
                let scope = runtime
                    .player
                    .scopes
                    .get_mut(token.slot())
                    .ok_or_else(super::cancelled_scope_error)?;
                scope.stack.push(result.clone());
            }
            Ok(())
        });
        match applied {
            Some(Ok(())) => {
                let stop = matches!(
                    &effect,
                    InternalResultEffect::Global {
                        stop_current_handler: true,
                        ..
                    }
                        | InternalResultEffect::Tell {
                            target: None,
                            is_return: true,
                            ..
                        }
                );
                if stop {
                    self.finish_current_frame(session)
                } else {
                    self.advance_current_opcode(session, &frame_ctx, &token)
                }
            }
            Some(Err(error)) => self.fail_current_frame(session, error),
            None => {
                self.release_all_ext_call_layers(session);
                self.phase = DriverPhase::Cancelled;
                DriverTurn::Error(super::cancelled_scope_error())
            }
        }
    }

    fn dispatch_set_property(
        &mut self,
        session: &mut RuntimeSession,
        frame_ctx: &BytecodeHandlerContext,
        receiver: DatumRef,
        name: Symbol,
        value: DatumRef,
    ) -> DriverTurn {
        let mut outbox = CastNotificationOutbox::default();
        let outcome = Self::with_context(session, self.player_id, |runtime| {
            if !frame_ctx.scope.validate_top(runtime.player) {
                return Err(super::cancelled_scope_error());
            }
            set_obj_prop_sync(
                runtime.player,
                runtime.symbols,
                &receiver,
                name,
                &value,
                &mut outbox,
            )
        });
        let Some(outcome) = outcome else {
            self.phase = DriverPhase::Cancelled;
            return DriverTurn::Error(super::cancelled_scope_error());
        };
        session.append_notifications(self.player_id, &mut outbox);
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => return self.fail_current_frame(session, error),
        };
        match outcome {
            SetObjPropOutcome::Applied => {
                self.advance_current_opcode(session, frame_ctx, &frame_ctx.scope)
            }
            SetObjPropOutcome::JsObject { receiver, name, value } => {
                let Some(owner) = Self::with_context(session, self.player_id, |runtime| {
                    runtime.player.owner.clone()
                }) else {
                    return self.fail_current_frame(session, super::cancelled_scope_error());
                };
                let Some(ticket) = session.allocate_action(
                    &owner,
                    &frame_ctx.scope,
                    ActionKind::InternalInvocation,
                    ResumePhase::ApplyOpcode,
                ) else {
                    return self.fail_current_frame(
                        session,
                        ScriptError::new("driver action sequence exhausted".to_owned()),
                    );
                };
                self.internal_effect = Some(InternalResultEffect::SetProperty);
                self.phase = DriverPhase::Awaiting {
                    ticket: ticket.clone(),
                    resume: ResumePhase::ApplyOpcode,
                };
                DriverTurn::Pending(PendingAction::Internal(InternalInvocationRequest {
                    ticket,
                    scope: frame_ctx.scope.clone(),
                    request: InternalVmRequest::SetProperty { receiver, name, value },
                    pending_reason: Some("JS object property setter requires owner-bound runtime execution".to_owned()),
                    effect: Some(InternalResultEffect::SetProperty),
                }))
            }
            SetObjPropOutcome::FlashSet(request) => {
                let owner_live = Self::with_context(session, self.player_id, |runtime| {
                    runtime.player.owner.same_identity(&request.owner)
                        && runtime.player.owner.is_arena_live()
                })
                .unwrap_or(false);
                if !owner_live {
                    return self.fail_current_frame(
                        session,
                        ScriptError::new_code(
                            super::ScriptErrorCode::InvalidReference,
                            "Flash property owner was replaced".to_owned(),
                        ),
                    );
                }
                let Some(ticket) = session.allocate_action(
                    &request.owner,
                    &frame_ctx.scope,
                    ActionKind::InternalInvocation,
                    ResumePhase::ApplyOpcode,
                ) else {
                    return self.fail_current_frame(
                        session,
                        ScriptError::new("driver action sequence exhausted".to_owned()),
                    );
                };
                self.internal_effect = Some(InternalResultEffect::SetProperty);
                self.phase = DriverPhase::Awaiting {
                    ticket: ticket.clone(),
                    resume: ResumePhase::ApplyOpcode,
                };
                DriverTurn::Pending(PendingAction::Internal(InternalInvocationRequest {
                    ticket,
                    scope: frame_ctx.scope.clone(),
                    request: InternalVmRequest::Flash(request),
                    pending_reason: Some("Flash property setter requires owner-bound host execution".to_owned()),
                    effect: Some(InternalResultEffect::SetProperty),
                }))
            }
            SetObjPropOutcome::AwaitCastLoad(request) => {
                let Some(owner) = Self::with_context(session, self.player_id, |runtime| {
                    runtime.player.owner.clone()
                }) else {
                    return self.fail_current_frame(session, super::cancelled_scope_error());
                };
                let Some(ticket) = session.allocate_action(
                    &owner,
                    &frame_ctx.scope,
                    ActionKind::InternalInvocation,
                    ResumePhase::ApplyOpcode,
                ) else {
                    return self.fail_current_frame(
                        session,
                        ScriptError::new("driver action sequence exhausted".to_owned()),
                    );
                };
                let outbound = match session.begin_property_cast_load(
                    self.player_id,
                    request,
                    ticket.clone(),
                ) {
                    Ok(outbound) => outbound,
                    Err(error) => return self.fail_current_frame(session, error),
                };
                if let Some(request) = outbound {
                    self.internal_effect = Some(InternalResultEffect::SetProperty);
                    self.phase = DriverPhase::Awaiting {
                        ticket: ticket.clone(),
                        resume: ResumePhase::ApplyOpcode,
                    };
                    DriverTurn::Pending(PendingAction::CastLoad { ticket, request })
                } else {
                    session.discard_completed_property_cast(&ticket);
                    self.advance_current_opcode(session, frame_ctx, &frame_ctx.scope)
                }
            }
        }
    }


    fn continue_global_dispatch(
        &mut self,
        session: &mut RuntimeSession,
        frame_ctx: &BytecodeHandlerContext,
        effect: InternalResultEffect,
        ext_call_layer: bool,
        base_request: InternalVmRequest,
        dispatch: GlobalDispatch,
    ) -> DriverTurn {
        let child_args = match &base_request {
            InternalVmRequest::Global { args, .. }
            | InternalVmRequest::GlobalAfterExternalProbe { args, .. } => args.clone(),
            InternalVmRequest::Object { receiver, args, .. }
            | InternalVmRequest::ObjectV4 { receiver, args, .. } => {
                let mut child_args = Vec::with_capacity(args.len() + 1);
                child_args.push(receiver.clone());
                child_args.extend(args.iter().cloned());
                child_args
            }
            _ => Vec::new(),
        };
        let (request, pending_reason) = match dispatch {
                GlobalDispatch::Child {
                    receiver,
                    handler_ref,
                } => {
                    let push_return = match &effect {
                        InternalResultEffect::Global { push_return, .. }
                        | InternalResultEffect::Tell { push_return, .. }
                        | InternalResultEffect::ObjectV4 { push_return, .. } => *push_return,
                        _ => false,
                    };
                    let policy = if matches!(
                        &effect,
                        InternalResultEffect::ObjectV4 {
                            route_to_global: true,
                            ..
                        }
                    ) {
                        Some(ChildReturnPolicy::ObjCallV4)
                    } else {
                        Some(ChildReturnPolicy::GlobalScope {
                            ext_call_layer,
                        })
                    };
                    self.internal_effect = None;
                    self.internal_ext_call_layer = false;
                    return self.start_child_with_policy(
                        session,
                        super::PendingCall {
                            receiver,
                            handler_ref,
                            args: child_args,
                            use_raw_arg_list: true,
                            push_return,
                        },
                        policy,
                    );
                }
                GlobalDispatch::ChildPrepared { receiver, handler_ref, args: child_args } => {
                    let push_return = match &effect {
                        InternalResultEffect::Global { push_return, .. }
                        | InternalResultEffect::Tell { push_return, .. }
                        | InternalResultEffect::ObjectV4 { push_return, .. } => *push_return,
                        _ => false,
                    };
                    let policy = if matches!(&effect, InternalResultEffect::ObjectV4 { route_to_global: true, .. }) {
                        Some(ChildReturnPolicy::ObjCallV4)
                    } else {
                        Some(ChildReturnPolicy::GlobalScope { ext_call_layer })
                    };
                    self.internal_effect = None;
                    self.internal_ext_call_layer = false;
                    return self.start_child_with_policy(
                        session,
                        super::PendingCall {
                            receiver,
                            handler_ref,
                            args: child_args,
                            use_raw_arg_list: false,
                            push_return,
                        },
                        policy,
                    );
                }
                GlobalDispatch::ChildWithCompletion { receiver, handler_ref, args, completion } => {
                    let push_return = match &effect {
                        InternalResultEffect::Global { push_return, .. }
                        | InternalResultEffect::Tell { push_return, .. }
                        | InternalResultEffect::ObjectV4 { push_return, .. } => *push_return,
                        _ => false,
                    };
                    let delivery = match &effect {
                        InternalResultEffect::Global { .. }
                        | InternalResultEffect::Tell { .. } => Some(ChildDelivery::GlobalScope),
                        InternalResultEffect::ObjectV4 { route_to_global: true, .. } => {
                            Some(ChildDelivery::ObjCallV4)
                        }
                        _ => None,
                    };
                    self.internal_effect = None;
                    self.internal_ext_call_layer = false;
                    return self.start_child_with_policy(
                        session,
                        super::PendingCall { receiver, handler_ref, args, use_raw_arg_list: false, push_return },
                        Some(ChildReturnPolicy::Completion {
                            completion,
                            ext_call_layer,
                            delivery,
                        }),
                    );
                }
                GlobalDispatch::AncestorChildren { calls } => {
                    let mut remaining_calls = calls.into_iter();
                    let mut selected = None;
                    while let Some(call) = remaining_calls.next() {
                        match self.resolve_ancestor_pending(session, &call) {
                            Ok(Some(pending)) => {
                                selected = Some((pending, call));
                                break;
                            }
                            Ok(None) => {}
                            Err(error) => {
                                self.internal_effect = None;
                                if ext_call_layer {
                                    self.release_ext_call_layer(session);
                                }
                                return self.fail_current_frame(session, error);
                            }
                        }
                    }
                    let Some((mut pending, _call)) = selected else {
                        self.internal_effect = None;
                        if ext_call_layer { self.release_ext_call_layer(session); }
                        return self.apply_internal_result(session, &frame_ctx, &frame_ctx.scope, effect, DatumRef::Void);
                    };
                    let push_return = match &effect {
                        InternalResultEffect::Global { push_return, .. }
                        | InternalResultEffect::Tell { push_return, .. }
                        | InternalResultEffect::ObjectV4 { push_return, .. } => *push_return,
                        _ => false,
                    };
                    let delivery = match &effect {
                        InternalResultEffect::Global { .. }
                        | InternalResultEffect::Tell { .. } => Some(ChildDelivery::GlobalScope),
                        InternalResultEffect::ObjectV4 { route_to_global: true, .. } => {
                            Some(ChildDelivery::ObjCallV4)
                        }
                        _ => None,
                    };
                    let remaining: Vec<_> = remaining_calls.collect();
                    pending.push_return = remaining.is_empty() && push_return;
                    self.internal_effect = None;
                    self.internal_ext_call_layer = false;
                    return self.start_child_with_policy(
                        session,
                        pending,
                        Some(ChildReturnPolicy::Ancestor {
                            remaining,
                            final_push_return: push_return,
                            ext_call_layer,
                            delivery,
                        }),
                    );
                }
                GlobalDispatch::ChildSequence { plan } => {
                    let BroadcastPlan { calls, fallback, initial_return, handled: initial_handled, continue_on_error } = plan;
                    let push_return = match &effect {
                        InternalResultEffect::Global { push_return, .. }
                        | InternalResultEffect::Tell { push_return, .. }
                        | InternalResultEffect::ObjectV4 { push_return, .. } => *push_return,
                        _ => false,
                    };
                    let delivery = match &effect {
                        InternalResultEffect::Global { .. }
                        | InternalResultEffect::Tell { .. } => Some(ChildDelivery::GlobalScope),
                        InternalResultEffect::ObjectV4 { route_to_global: true, .. } => {
                            Some(ChildDelivery::ObjCallV4)
                        }
                        _ => None,
                    };
                    let mut remaining_calls = calls.into_iter();
                    let Some(first_call) = remaining_calls.next() else {
                        // No behavior receiver was prepared.  Try the static
                        // frame/movie chain before completing the opcode.
                        let mut remaining_static = fallback.into_iter();
                        while let Some(static_call) = remaining_static.next() {
                            match self.resolve_static_pending(session, &static_call) {
                                Ok(Some(mut pending)) => {
                                    let remaining_static: Vec<_> = remaining_static.collect();
                                    pending.push_return = remaining_static.is_empty() && push_return;
                                    self.internal_effect = None;
                                    self.internal_ext_call_layer = false;
                                    return self.start_child_with_policy(
                                        session,
                                        pending,
                                        Some(ChildReturnPolicy::Broadcast {
                                            remaining: Vec::new(),
                                            fallback: remaining_static,
                                            final_push_return: push_return,
                                            ext_call_layer,
                                            delivery,
                                            last_return_value: initial_return.clone(),
                                            handled: initial_handled,
                                            continue_on_error,
                                        }),
                                    );
                                }
                                Ok(None) => {}
                                Err(error) => {
                                    self.internal_effect = None;
                                    if ext_call_layer { self.release_ext_call_layer(session); }
                                    return self.fail_current_frame(session, error);
                                }
                            }
                        }
                        self.internal_effect = None;
                        if ext_call_layer { self.release_ext_call_layer(session); }
                        return self.apply_internal_result(
                            session, &frame_ctx, &frame_ctx.scope, effect, initial_return,
                        );
                    };
                    let pending = match self.resolve_broadcast_pending(session, &first_call) {
                        Ok(Some(pending)) => pending,
                        Ok(None) => {
                            // A receiver may have acquired a handler after
                            // preparation.  Keep walking behavior receivers
                            // before considering static fallback.
                            while let Some(call) = remaining_calls.next() {
                                match self.resolve_broadcast_pending(session, &call) {
                                    Ok(Some(mut pending)) => {
                                        let remaining: Vec<_> = remaining_calls.collect();
                                        pending.push_return = remaining.is_empty() && fallback.is_empty() && push_return;
                                        self.internal_effect = None;
                                        self.internal_ext_call_layer = false;
                                        return self.start_child_with_policy(
                                            session,
                                            pending,
                                            Some(ChildReturnPolicy::Broadcast {
                                                remaining,
                                                fallback: fallback.clone(),
                                                final_push_return: push_return,
                                                ext_call_layer,
                                                delivery,
                                                last_return_value: initial_return.clone(),
                                                handled: initial_handled,
                                                continue_on_error,
                                            }),
                                        );
                                    }
                                    Ok(None) => {}
                                    Err(error) => {
                                        self.internal_effect = None;
                                        if ext_call_layer { self.release_ext_call_layer(session); }
                                        return self.fail_current_frame(session, error);
                                    }
                                }
                            }
                            let mut remaining_static = fallback.into_iter();
                            while let Some(static_call) = remaining_static.next() {
                                match self.resolve_static_pending(session, &static_call) {
                                    Ok(Some(mut pending)) => {
                                        let fallback: Vec<_> = remaining_static.collect();
                                        pending.push_return = fallback.is_empty() && push_return;
                                        self.internal_effect = None;
                                        self.internal_ext_call_layer = false;
                                        return self.start_child_with_policy(
                                            session,
                                            pending,
                                            Some(ChildReturnPolicy::Broadcast {
                                                remaining: Vec::new(),
                                                fallback,
                                                final_push_return: push_return,
                                                ext_call_layer,
                                                delivery,
                                                last_return_value: initial_return.clone(),
                                                handled: initial_handled,
                                                continue_on_error,
                                            }),
                                        );
                                    }
                                    Ok(None) => {}
                                    Err(error) => {
                                        self.internal_effect = None;
                                        if ext_call_layer { self.release_ext_call_layer(session); }
                                        return self.fail_current_frame(session, error);
                                    }
                                }
                            }
                            self.internal_effect = None;
                            if ext_call_layer { self.release_ext_call_layer(session); }
                            return self.apply_internal_result(
                                session,
                                &frame_ctx,
                                &frame_ctx.scope,
                                effect,
                                initial_return,
                            );
                        }
                        Err(error) => {
                            self.internal_effect = None;
                            if ext_call_layer {
                                self.release_ext_call_layer(session);
                            }
                            return self.fail_current_frame(session, error);
                        }
                    };
                    let mut pending = pending;
                    let remaining: Vec<_> = remaining_calls.collect();
                    pending.push_return = remaining.is_empty() && fallback.is_empty() && push_return;
                    self.internal_effect = None;
                    self.internal_ext_call_layer = false;
                    return self.start_child_with_policy(
                        session,
                        pending,
                        Some(ChildReturnPolicy::Broadcast {
                            remaining,
                            fallback,
                            final_push_return: push_return,
                            ext_call_layer,
                            delivery,
                            last_return_value: initial_return,
                            handled: initial_handled,
                            continue_on_error,
                        }),
                    );
                }
                GlobalDispatch::SyncResult(result) => {
                    self.internal_effect = None;
                    if ext_call_layer {
                        self.release_ext_call_layer(session);
                    }
                    let result = match result {
                        Ok(result) => result,
                        Err(error) => return self.fail_current_frame(session, error),
                    };
                    return self.apply_internal_result(
                        session,
                        &frame_ctx,
                        &frame_ctx.scope,
                        effect,
                        result,
                    );
                }
                GlobalDispatch::PendingRequest { request: prepared, reason } => {
                    (prepared, Some(reason))
                }
                GlobalDispatch::Pending { reason } => {
                    let request = match base_request {
                        InternalVmRequest::GlobalAfterExternalProbe { name, args, .. }
                        | InternalVmRequest::Global { name, args } => {
                            match session.prepare_async_global_request(self.player_id, &name, &args) {
                                Ok(Some(request)) => request,
                                Ok(None) => InternalVmRequest::Global { name, args },
                                Err(error) => {
                                    if ext_call_layer {
                                        self.release_ext_call_layer(session);
                                    }
                                    return self.fail_current_frame(session, error);
                                }
                            }
                        }
                        request => request,
                    };
                    (request, Some(reason))
                }
            };
        let Some(owner) = Self::with_context(session, self.player_id, |ctx| {
            ctx.player.owner.clone()
        }) else {
            if ext_call_layer {
                self.release_ext_call_layer(session);
            }
            self.phase = DriverPhase::Cancelled;
            return DriverTurn::Error(super::cancelled_scope_error());
        };
        let Some(ticket) = session.allocate_action(
            &owner,
            &frame_ctx.scope,
            ActionKind::InternalInvocation,
            ResumePhase::ApplyOpcode,
        ) else {
            if ext_call_layer {
                self.release_ext_call_layer(session);
            }
            let error = ScriptError::new("driver action sequence exhausted".to_owned());
            self.phase = DriverPhase::Failed(error.clone());
            return DriverTurn::Error(error);
        };
        self.internal_effect = Some(effect.clone());
        self.internal_ext_call_layer = ext_call_layer;
        let action = PendingAction::Internal(InternalInvocationRequest {
            ticket: ticket.clone(),
            scope: frame_ctx.scope.clone(),
            request,
            pending_reason,
            effect: Some(effect),
        });
        self.phase = DriverPhase::Awaiting {
            ticket,
            resume: ResumePhase::ApplyOpcode,
        };
        DriverTurn::Pending(action)
    }

    fn prepare_internal_action(
        &mut self,
        session: &mut RuntimeSession,
        opcode: OpCode,
        frame_ctx: BytecodeHandlerContext,
    ) -> DriverTurn {
        // Decode arguments before entering the legacy layer. Malformed
        // ExtCall/TellCall bytecode historically fails before player_ext_call
        // increments handler depth.
        let decoded = Self::with_context(session, self.player_id, |runtime| {
            if !frame_ctx.scope.validate_top(runtime.player) {
                return Err(super::cancelled_scope_error());
            }
            match opcode {
                OpCode::GetObjProp => {
                    let bytecode = runtime.player.get_ctx_current_bytecode(&frame_ctx);
                    let name = checked_context_name(&frame_ctx, bytecode.obj as u16)?;
                    runtime.symbols.display(&name).map_err(|_| {
                        ScriptError::new_code(
                            super::ScriptErrorCode::InvalidReference,
                            "foreign or stale opcode symbol".to_owned(),
                        )
                    })?;
                    let receiver = pop_internal_ref(runtime, &frame_ctx, "get_obj_prop receiver")?;
                    let receiver_value = checked_internal_datum(runtime.player, runtime.symbols, &receiver)?.clone();
                    let request = match receiver_value {
                        Datum::String(_) | Datum::StringChunk(..)
                            if name == Symbol::builtin(BuiltInSymbol::Value) =>
                        {
                            InternalVmRequest::EvaluateValue {
                                source: receiver,
                                mode: ValueEvaluationMode::StringPropertyFallback,
                            }
                        }
                        Datum::FlashObjectRef(_) => InternalVmRequest::Flash(
                            crate::player::handlers::datum_handlers::flash_object::FlashObjectDatumHandlers::prepare_get_prop(
                                runtime.player,
                                &receiver,
                                &runtime.symbols.display(&name).map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?.to_owned(),
                            )?,
                        ),
                        _ => InternalVmRequest::ObjectProperty { receiver, name },
                    };
                    Ok((
                        request,
                        InternalResultEffect::ObjectV4 {
                            push_return: true,
                            route_to_global: false,
                        },
                        None,
                    ))
                }
                OpCode::NewObj => {
                    let bytecode = runtime.player.get_ctx_current_bytecode(&frame_ctx);
                    let name = checked_context_name(&frame_ctx, bytecode.obj as u16)?;
                    runtime.symbols.display(&name).map_err(|_| {
                        ScriptError::new_code(
                            super::ScriptErrorCode::InvalidReference,
                            "foreign or stale opcode symbol".to_owned(),
                        )
                    })?;
                    if name != BuiltInSymbol::Script {
                        return Err(ScriptError::new(format!(
                            "Cannot create new instance of non-script: {}",
                            runtime.symbols.display(&name).unwrap_or("<foreign>")
                        )));
                    }
                    let (mut all_args, _) = pop_internal_call_args(runtime, &frame_ctx, opcode)?;
                    if all_args.is_empty() {
                        return Err(ScriptError::new(
                            "new_obj: arg list has no script".to_owned(),
                        ));
                    }
                    let script_arg_ref = all_args.remove(0);
                    let script_arg = checked_internal_datum(
                        runtime.player,
                        runtime.symbols,
                        &script_arg_ref,
                    )?
                    .clone();
                    let script = match script_arg {
                        Datum::String(script_name) => runtime
                            .player
                            .movie
                            .cast_manager
                            .find_member_ref_by_name(&script_name)
                            .map(|member_ref| {
                                runtime
                                    .player
                                    .alloc_datum(Datum::ScriptRef(member_ref))
                            })
                            .ok_or_else(|| {
                                ScriptError::new(format!(
                                    "No script found with name {}",
                                    script_name
                                ))
                            })?,
                        Datum::CastMember(member_ref) => runtime
                            .player
                            .alloc_datum(Datum::ScriptRef(member_ref)),
                        _ => {
                            return Err(ScriptError::new(
                                "First argument to new script must be script name or CastMember"
                                    .to_owned(),
                            ))
                        }
                    };
                    for arg in &all_args {
                        checked_internal_datum(runtime.player, runtime.symbols, arg)?;
                    }
                    Ok((
                        InternalVmRequest::Construct {
                            script,
                            args: all_args,
                        },
                        InternalResultEffect::Construct,
                        None,
                    ))
                }
                OpCode::ExtCall => {
                    let bytecode = runtime.player.get_ctx_current_bytecode(&frame_ctx);
                    let name = checked_context_name(&frame_ctx, bytecode.obj as u16)?;
                    runtime.symbols.display(&name).map_err(|_| {
                        ScriptError::new_code(
                            super::ScriptErrorCode::InvalidReference,
                            "foreign or stale opcode symbol".to_owned(),
                        )
                    })?;
                    let (args, no_ret) = pop_internal_call_args(runtime, &frame_ctx, opcode)?;
                    for arg in &args {
                        checked_internal_datum(runtime.player, runtime.symbols, arg)?;
                    }
                    let is_return = name == BuiltInSymbol::Return;
                    let return_value = if is_return {
                        Some(args.first().cloned().unwrap_or(DatumRef::Void))
                    } else {
                        None
                    };
                    Ok((
                        InternalVmRequest::Global {
                            name,
                            args,
                        },
                        InternalResultEffect::Global {
                            push_return: !no_ret,
                            stop_current_handler: is_return,
                            return_value,
                        },
                        None,
                    ))
                }
                OpCode::ObjCallV4 => {
                    let handler_ref =
                        pop_internal_ref(runtime, &frame_ctx, "obj_call_v4 handler name")?;
                    let (mut all_args, no_ret) =
                        pop_internal_call_args(runtime, &frame_ctx, opcode)?;
                    let handler_datum = checked_internal_datum(
                        runtime.player,
                        runtime.symbols,
                        &handler_ref,
                    )?
                    .clone();
                    let handler_name = handler_datum.symbol_value(runtime.symbols)?;
                    if all_args.is_empty() {
                        return Err(ScriptError::new(
                            "obj_call_v4: arg list has no receiver".to_owned(),
                        ));
                    }
                    let mut receiver = all_args.remove(0);
                    let receiver_value = checked_internal_datum(
                        runtime.player,
                        runtime.symbols,
                        &receiver,
                    )?
                    .clone();
                    if let Datum::Symbol(sym_name) = receiver_value {
                        if let Some(global_ref) = runtime.player.globals.get(&sym_name) {
                            receiver = global_ref.clone();
                        }
                    }
                    checked_internal_datum(runtime.player, runtime.symbols, &receiver)?;
                    for arg in &all_args {
                        checked_internal_datum(runtime.player, runtime.symbols, arg)?;
                    }
                    let is_object_receiver = matches!(
                        checked_internal_datum(runtime.player, runtime.symbols, &receiver)?,
                        Datum::ScriptInstanceRef(_)
                            | Datum::ScriptRef(_)
                            | Datum::JsObjectRef(_)
                    );
                    let route_to_global = !is_object_receiver
                        && global_handler_exists(
                            runtime.player,
                            runtime.symbols,
                            &handler_name,
                        )?;
                    Ok((
                        InternalVmRequest::ObjectV4 {
                            receiver,
                            name: handler_name,
                            args: all_args,
                        },
                        InternalResultEffect::ObjectV4 {
                            push_return: !no_ret,
                            route_to_global,
                        },
                        None,
                    ))
                }
                OpCode::SetObjProp => {
                    let bytecode = runtime.player.get_ctx_current_bytecode(&frame_ctx);
                    let name = checked_context_name(&frame_ctx, bytecode.obj as u16)?;
                    runtime.symbols.display(&name).map_err(|_| {
                        ScriptError::new_code(
                            super::ScriptErrorCode::InvalidReference,
                            "foreign or stale opcode symbol".to_owned(),
                        )
                    })?;
                    let value = pop_internal_ref(runtime, &frame_ctx, "set_obj_prop value")?;
                    let receiver =
                        pop_internal_ref(runtime, &frame_ctx, "set_obj_prop receiver")?;
                    Ok((
                        InternalVmRequest::SetProperty {
                            receiver,
                            name,
                            value,
                        },
                        InternalResultEffect::SetProperty,
                        None,
                    ))
                }
                OpCode::TellCall => {
                    let bytecode = runtime.player.get_ctx_current_bytecode(&frame_ctx);
                    let name = checked_context_name(&frame_ctx, bytecode.obj as u16)?;
                    runtime.symbols.display(&name).map_err(|_| {
                        ScriptError::new_code(
                            super::ScriptErrorCode::InvalidReference,
                            "foreign or stale opcode symbol".to_owned(),
                        )
                    })?;
                    let (args, no_ret) = pop_internal_call_args(runtime, &frame_ctx, opcode)?;
                    for arg in &args {
                        checked_internal_datum(runtime.player, runtime.symbols, arg)?;
                    }
                    let target_id = runtime
                        .player
                        .tell_target_stack
                        .last()
                        .and_then(|target| target.nested_player)
                        .map(|id| {
                            PlayerId::try_from(id).map_err(|_| {
                                ScriptError::new_code(
                                    super::ScriptErrorCode::InvalidReference,
                                    "tell target player id overflow".to_owned(),
                                )
                            })
                        })
                        .transpose()?;
                    let is_return = target_id.is_none() && name == BuiltInSymbol::Return;
                    let return_value = if is_return {
                        Some(args.first().cloned().unwrap_or(DatumRef::Void))
                    } else {
                        None
                    };
                    Ok((
                        InternalVmRequest::Tell {
                            target: None,
                            name,
                            args,
                        },
                        InternalResultEffect::Tell {
                            push_return: !no_ret,
                            target: None,
                            is_return,
                            return_value,
                        },
                        target_id,
                    ))
                }
                _ => unreachable!("only async opcodes reach internal action preparation"),
            }
        });
        let Some(decoded) = decoded else {
            self.phase = DriverPhase::Cancelled;
            return DriverTurn::Error(super::cancelled_scope_error());
        };
        let (mut request, mut effect, target_id) = match decoded {
            Ok(decoded) => decoded,
            Err(error) => {
                return self.fail_current_frame(session, error);
            }
        };
        if let InternalVmRequest::SetProperty {
            receiver,
            name,
            value,
        } = &request
        {
            return self.dispatch_set_property(
                session,
                &frame_ctx,
                receiver.clone(),
                name.clone(),
                value.clone(),
            );
        }
        if let Some(target_id) = target_id {
            let Some(target_owner) = Self::with_context(session, target_id, |runtime| {
                runtime.player.owner.clone()
            }) else {
                return self.fail_current_frame(
                    session,
                    ScriptError::new_code(
                        super::ScriptErrorCode::InvalidReference,
                        "tell target player was removed".to_owned(),
                    ),
                );
            };
            if !target_owner.is_arena_live() {
                return self.fail_current_frame(
                    session,
                    ScriptError::new_code(
                        super::ScriptErrorCode::InvalidReference,
                        "tell target owner is no longer live".to_owned(),
                    ),
                );
            }
            let target = NestedTargetIdentity {
                player_id: target_id,
                owner: target_owner,
            };
            if let InternalVmRequest::Tell { target: slot, .. } = &mut request {
                *slot = Some(target.clone());
            }
            if let InternalResultEffect::Tell { target: slot, .. } = &mut effect {
                *slot = Some(target);
            }
        }

        let ext_call_layer = match opcode {
            OpCode::ExtCall => true,
            // A local TellCall shares player_ext_call's legacy accounting.
            // Cross-player Tell is owned by the nested executor instead.
            OpCode::TellCall => target_id.is_none(),
            _ => false,
        };
        if ext_call_layer && !self.enter_ext_call_layer(session, &frame_ctx.scope) {
            return self.fail_current_frame(
                session,
                ScriptError::new("could not enter ExtCall handler layer".to_owned()),
            );
        }

        // Global calls are classified synchronously while the session owns
        // the player. A bytecode child is mounted directly on this driver's
        // frame stack; virtual/sync results are applied in this turn. Only an
        // explicitly asynchronous implementation remains an owned pending
        // request for the external loop.
        let mut pending_reason = target_id
            .is_some()
            .then(|| "cross-player Tell requires nested executor".to_owned());
        let global_args = match &request {
            InternalVmRequest::Global { name, args } => Some((name.clone(), args.clone())),
            InternalVmRequest::Tell {
                target: None,
                name,
                args,
            } => Some((name.clone(), args.clone())),
            InternalVmRequest::ObjectV4 {
                receiver,
                name,
                args,
            } if matches!(
                &effect,
                InternalResultEffect::ObjectV4 {
                    route_to_global: true,
                    ..
                }
            ) => {
                let mut args_with_receiver = Vec::with_capacity(args.len() + 1);
                args_with_receiver.push(receiver.clone());
                args_with_receiver.extend(args.iter().cloned());
                Some((name.clone(), args_with_receiver))
            }
            _ => None,
        };
        if let Some((name, args)) = global_args {
            let forced_result = match &effect {
                InternalResultEffect::Global {
                    return_value: Some(value),
                    ..
                }
                | InternalResultEffect::Tell {
                    return_value: Some(value),
                    ..
                } => Some(value.clone()),
                _ => None,
            };
            if let Some(result) = forced_result {
                self.internal_effect = None;
                if ext_call_layer {
                    self.release_ext_call_layer(session);
                }
                return self.apply_internal_result(
                    session,
                    &frame_ctx,
                    &frame_ctx.scope,
                    effect,
                    result,
                );
            }
            let dispatch = match session.dispatch_global(self.player_id, &name, &args) {
                Ok(dispatch) => dispatch,
                Err(error) => {
                    if ext_call_layer {
                        self.release_ext_call_layer(session);
                    }
                    return self.fail_current_frame(session, error);
                }
            };
            return self.continue_global_dispatch(
                session,
                &frame_ctx,
                effect,
                ext_call_layer,
                request,
                dispatch,
            );
        }

        // Non-global internal requests continue through the ordinary pending
        // action path below.
        let Some(owner) = Self::with_context(session, self.player_id, |ctx| {
            ctx.player.owner.clone()
        }) else {
            if ext_call_layer {
                self.release_ext_call_layer(session);
            }
            self.phase = DriverPhase::Cancelled;
            return DriverTurn::Error(super::cancelled_scope_error());
        };
        let Some(ticket) = session.allocate_action(
            &owner,
            &frame_ctx.scope,
            ActionKind::InternalInvocation,
            ResumePhase::ApplyOpcode,
        ) else {
            if ext_call_layer {
                self.release_ext_call_layer(session);
            }
            let error = ScriptError::new("action sequence exhausted".to_owned());
            self.phase = DriverPhase::Failed(error.clone());
            return DriverTurn::Error(error);
        };
        self.internal_effect = Some(effect.clone());
        self.internal_ext_call_layer = ext_call_layer;
        let action = PendingAction::Internal(InternalInvocationRequest {
            ticket: ticket.clone(),
            scope: frame_ctx.scope,
            request,
            pending_reason,
            effect: Some(effect),
        });
        self.phase = DriverPhase::Awaiting {
            ticket,
            resume: ResumePhase::ApplyOpcode,
        };
        DriverTurn::Pending(action)
    }

    fn finish_current_frame(&mut self, session: &mut RuntimeSession) -> DriverTurn {
        let static_event_was_active = self.static_event_guard.is_some();
        self.release_static_event_guard(session);
        let Some(frame) = self.frames.last() else {
            self.phase = DriverPhase::Completing;
            return DriverTurn::Complete(ScopeResult {
                return_value: DatumRef::Void,
                passed: false,
            });
        };
        let child_policy = self
            .child_return_policies
            .last()
            .cloned()
            .flatten();
        let result_valid = Self::with_context(session, self.player_id, |runtime| {
            if !frame.ctx.scope.validate_top(runtime.player) {
                return Err(super::cancelled_scope_error());
            }
            let Some(scope) = runtime.player.scopes.get(frame.ctx.scope.slot()) else {
                return Err(super::cancelled_scope_error());
            };
            validate_obj_call_result(runtime, &scope.return_value)
        });
        match result_valid {
            Some(Ok(())) => {}
            Some(Err(error)) => return self.fail_current_frame(session, error),
            None => {
                self.phase = DriverPhase::Cancelled;
                return DriverTurn::Error(super::cancelled_scope_error());
            }
        }
        let result = Self::with_context(session, self.player_id, |runtime| {
            super::teardown_handler_frame(runtime.player, &frame.ctx.scope, frame.is_frame_script)
        });
        match result {
            Some(Ok(mut scope_result)) => {
                // Keep the frame at the front of cancellation until teardown
                // has validated and popped its scope. A stale child must not
                // expose a valid parent to a subsequent unwind.
                let frame = self.frames.pop().expect("current frame was retained");
                self.child_return_policies.pop();
                // Ancestor callbacks are ordered children of one parent
                // invocation. An early/empty callback must still advance to
                // the next prepared child before delivering a result; retain
                // the ExtCall layer until the final child returns.
                if let Some(ChildReturnPolicy::Ancestor {
                    mut remaining,
                    final_push_return,
                    ext_call_layer,
                    delivery,
                }) = child_policy.clone()
                {
                    let mut remaining_calls = remaining.into_iter();
                    while let Some(call) = remaining_calls.next() {
                        match self.resolve_ancestor_pending(session, &call) {
                            Ok(Some(mut pending)) => {
                                let remaining: Vec<_> = remaining_calls.collect();
                                pending.push_return = remaining.is_empty() && final_push_return;
                                return self.start_child_without_advancing(
                                    session,
                                    pending,
                                    Some(ChildReturnPolicy::Ancestor {
                                        remaining,
                                        final_push_return,
                                        ext_call_layer,
                                        delivery,
                                    }),
                                );
                            }
                            Ok(None) => {}
                            Err(error) => return self.fail_current_frame(session, error),
                        }
                    }
                }
                if let Some(ChildReturnPolicy::Broadcast {
                    remaining,
                    fallback,
                    final_push_return,
                    ext_call_layer,
                    delivery,
                    mut last_return_value,
                    handled,
                    continue_on_error,
                }) = child_policy.clone()
                {
                let handled = handled
                    || !scope_result.passed
                    || (static_event_was_active && session.static_event_stopped(self.player_id));
                    if scope_result.return_value != DatumRef::Void {
                        last_return_value = scope_result.return_value.clone();
                    }
                    let mut remaining_calls = remaining.into_iter();
                    while let Some(call) = remaining_calls.next() {
                        match self.resolve_broadcast_pending(session, &call) {
                            Ok(Some(mut pending)) => {
                                let remaining: Vec<_> = remaining_calls.collect();
                                pending.push_return = remaining.is_empty() && fallback.is_empty() && final_push_return;
                                return self.start_child_without_advancing(
                                    session,
                                    pending,
                                    Some(ChildReturnPolicy::Broadcast {
                                        remaining,
                                        fallback: fallback.clone(),
                                        final_push_return,
                                        ext_call_layer,
                                        delivery,
                                        last_return_value,
                                        handled,
                                        continue_on_error,
                                    }),
                                );
                            }
                            Ok(None) => {}
                            Err(error) => return self.fail_current_frame(session, error),
                        }
                    }
                    if !handled {
                        let mut remaining_static = fallback.into_iter();
                        while let Some(static_call) = remaining_static.next() {
                            match self.resolve_static_pending(session, &static_call) {
                            Ok(Some(mut pending)) => {
                                let fallback: Vec<_> = remaining_static.collect();
                                pending.push_return = fallback.is_empty() && final_push_return;
                                return self.start_child_without_advancing(
                                    session,
                                    pending,
                                    Some(ChildReturnPolicy::Broadcast {
                                        remaining: Vec::new(),
                                        fallback,
                                        final_push_return,
                                        ext_call_layer,
                                        delivery,
                                        last_return_value,
                                        handled,
                                        continue_on_error,
                                    }),
                                );
                            }
                            Ok(None) => {}
                            Err(error) => return self.fail_current_frame(session, error),
                            }
                        }
                    }
                    scope_result.return_value = last_return_value;
                    scope_result.passed = !handled;
                }
                if Self::policy_has_ext_call_layer(child_policy.as_ref()) {
                    self.release_ext_call_layer(session);
                }
                if let Some(ChildReturnPolicy::Completion { completion, .. }) = &child_policy {
                    let transformed = Self::with_context(session, self.player_id, |runtime| {
                        match completion {
                            ChildCompletion::ConstructScript { fallback } =>
                                super::handlers::datum_handlers::script::ScriptDatumHandlers::finish_constructor(
                                    runtime.player, runtime.symbols, fallback.clone(), scope_result.return_value.clone(),
                                ),
                            ChildCompletion::TimeoutNew { timeout_name, fallback } =>
                                super::handlers::datum_handlers::timeout::TimeoutDatumHandlers::finish_new(
                                    runtime.player, runtime.symbols, timeout_name.clone(), fallback.clone(), scope_result.return_value.clone(),
                                ),
                            ChildCompletion::TimeoutForget { timeout_name } =>
                                super::handlers::datum_handlers::timeout::TimeoutDatumHandlers::finish_forget(
                                    runtime.player, runtime.symbols, timeout_name.clone(), scope_result.return_value.clone(),
                                ),
                        }
                    });
                    match transformed {
                        Some(Ok(result)) => scope_result.return_value = result,
                        Some(Err(error)) => return self.fail_current_frame(session, error),
                        None => {
                            self.phase = DriverPhase::Cancelled;
                            return DriverTurn::Error(super::cancelled_scope_error());
                        }
                    }
                }
                if self.frames.is_empty() {
                    self.phase = DriverPhase::Completing;
                    return DriverTurn::Complete(scope_result);
                }
                let parent = self.frames.last().unwrap();
                let delivered = Self::with_context(session, self.player_id, |runtime| {
                    if !parent.ctx.scope.validate_top(runtime.player) {
                        return false;
                    }
                    if Self::policy_updates_last_handler_result(child_policy.as_ref()) {
                        runtime.player.last_handler_result = scope_result.return_value.clone();
                    }
                    if Self::policy_is_global_scope(child_policy.as_ref()) {
                        let Some(scope) = runtime.player.scopes.get_mut(parent.ctx.scope.slot()) else {
                            return false;
                        };
                        scope.return_value = scope_result.return_value.clone();
                    }
                    super::deliver_scope_return(
                        runtime.player,
                        &parent.ctx.scope,
                        &scope_result,
                        frame.push_return,
                    )
                })
                .unwrap_or(false);
                if !delivered {
                    self.phase = DriverPhase::Cancelled;
                    DriverTurn::Error(super::cancelled_scope_error())
                } else {
                    self.phase = DriverPhase::Running;
                    DriverTurn::Waiting
                }
            }
            _ => {
                self.phase = DriverPhase::Cancelled;
                DriverTurn::Error(super::cancelled_scope_error())
            }
        }
    }

    fn start_child(&mut self, session: &mut RuntimeSession, pending: super::PendingCall) -> DriverTurn {
        self.start_child_with_policy(session, pending, None)
    }

    fn start_child_with_policy(
        &mut self,
        session: &mut RuntimeSession,
        pending: super::PendingCall,
        child_policy: Option<ChildReturnPolicy>,
    ) -> DriverTurn {
        self.start_child_with_policy_inner(session, pending, child_policy, true)
    }

    fn start_child_without_advancing(
        &mut self,
        session: &mut RuntimeSession,
        pending: super::PendingCall,
        child_policy: Option<ChildReturnPolicy>,
    ) -> DriverTurn {
        self.start_child_with_policy_inner(session, pending, child_policy, false)
    }

    fn start_child_with_policy_inner(
        &mut self,
        session: &mut RuntimeSession,
        pending: super::PendingCall,
        child_policy: Option<ChildReturnPolicy>,
        advance_parent: bool,
    ) -> DriverTurn {
        let Some(parent) = self.frames.last() else {
            self.release_static_event_guard(session);
            if Self::policy_has_ext_call_layer(child_policy.as_ref()) {
                self.release_ext_call_layer(session);
            }
            self.phase = DriverPhase::Cancelled;
            return DriverTurn::Error(super::cancelled_scope_error());
        };
        let parent_scope = parent.ctx.scope.clone();
        let parent_advanced = if !advance_parent { true } else {
            Self::with_context(session, self.player_id, |ctx| {
                if !parent_scope.validate_top(ctx.player) { return false; }
                let Some(scope) = ctx.player.scopes.get_mut(parent_scope.slot()) else { return false; };
                scope.bytecode_index = scope.bytecode_index.saturating_add(1);
                true
            }).unwrap_or(false)
        };
        if !parent_advanced {
            self.release_static_event_guard(session);
            if Self::policy_has_ext_call_layer(child_policy.as_ref()) {
                self.release_ext_call_layer(session);
            }
            self.phase = DriverPhase::Cancelled;
            return DriverTurn::Error(super::cancelled_scope_error());
        }
        let expectation = Self::with_context(session, self.player_id, |ctx| {
            super::SetupExpectation::capture(ctx.player, Some(&parent_scope))
        });
        let Some(expectation) = expectation else {
            self.release_static_event_guard(session);
            if Self::policy_has_ext_call_layer(child_policy.as_ref()) {
                self.release_ext_call_layer(session);
            }
            self.phase = DriverPhase::Cancelled;
            return DriverTurn::Error(super::cancelled_scope_error());
        };
        match super::setup_handler_frame(
            session,
            self.player_id,
            pending.receiver,
            pending.handler_ref,
            &pending.args,
            pending.use_raw_arg_list,
            pending.push_return,
            expectation,
            ) {
            Ok(super::FrameSetup::Early(mut result)) => {
                let static_event_was_active = self.static_event_guard.is_some();
                self.release_static_event_guard(session);
                if let Some(ChildReturnPolicy::Ancestor {
                    mut remaining,
                    final_push_return,
                    ext_call_layer,
                    delivery,
                }) = child_policy.clone()
                {
                    let mut remaining_calls = remaining.into_iter();
                    while let Some(call) = remaining_calls.next() {
                        match self.resolve_ancestor_pending(session, &call) {
                            Ok(Some(mut pending)) => {
                                let remaining: Vec<_> = remaining_calls.collect();
                                pending.push_return = remaining.is_empty() && final_push_return;
                                return self.start_child_without_advancing(
                                    session,
                                    pending,
                                    Some(ChildReturnPolicy::Ancestor {
                                        remaining,
                                        final_push_return,
                                        ext_call_layer,
                                        delivery,
                                    }),
                                );
                            }
                            Ok(None) => {}
                            Err(error) => return self.fail_current_frame(session, error),
                        }
                    }
                }
                if let Some(ChildReturnPolicy::Broadcast {
                    remaining,
                    fallback,
                    final_push_return,
                    ext_call_layer,
                    delivery,
                    mut last_return_value,
                    handled,
                    continue_on_error,
                }) = child_policy.clone()
                {
                    let handled = handled
                        || !result.passed
                        || (static_event_was_active && session.static_event_stopped(self.player_id));
                    if result.return_value != DatumRef::Void {
                        last_return_value = result.return_value.clone();
                    }
                    let mut remaining_calls = remaining.into_iter();
                    while let Some(call) = remaining_calls.next() {
                        match self.resolve_broadcast_pending(session, &call) {
                            Ok(Some(mut next_pending)) => {
                                let remaining: Vec<_> = remaining_calls.collect();
                                next_pending.push_return = remaining.is_empty() && fallback.is_empty() && final_push_return;
                                return self.start_child_without_advancing(
                                    session,
                                    next_pending,
                                    Some(ChildReturnPolicy::Broadcast {
                                        remaining,
                                        fallback: fallback.clone(),
                                        final_push_return,
                                        ext_call_layer,
                                        delivery,
                                        last_return_value,
                                        handled,
                                        continue_on_error,
                                    }),
                                );
                            }
                            Ok(None) => {}
                            Err(error) => return self.fail_current_frame(session, error),
                        }
                    }
                    if !handled {
                        let mut remaining_static = fallback.into_iter();
                        while let Some(static_call) = remaining_static.next() {
                            match self.resolve_static_pending(session, &static_call) {
                            Ok(Some(mut next_pending)) => {
                                let fallback: Vec<_> = remaining_static.collect();
                                next_pending.push_return = fallback.is_empty() && final_push_return;
                                return self.start_child_without_advancing(
                                    session,
                                    next_pending,
                                    Some(ChildReturnPolicy::Broadcast {
                                        remaining: Vec::new(),
                                        fallback,
                                        final_push_return,
                                        ext_call_layer,
                                        delivery,
                                        last_return_value,
                                        handled,
                                        continue_on_error,
                                    }),
                                );
                            }
                            Ok(None) => {}
                            Err(error) => return self.fail_current_frame(session, error),
                            }
                        }
                    }
                    result.return_value = last_return_value;
                    result.passed = !handled;
                }
                if Self::policy_has_ext_call_layer(child_policy.as_ref()) {
                    self.release_ext_call_layer(session);
                }
                if let Some(ChildReturnPolicy::Completion { completion, .. }) = &child_policy {
                    let transformed = Self::with_context(session, self.player_id, |runtime| {
                        match completion {
                            ChildCompletion::ConstructScript { fallback } =>
                                super::handlers::datum_handlers::script::ScriptDatumHandlers::finish_constructor(
                                    runtime.player, runtime.symbols, fallback.clone(), result.return_value.clone(),
                                ),
                            ChildCompletion::TimeoutNew { timeout_name, fallback } =>
                                super::handlers::datum_handlers::timeout::TimeoutDatumHandlers::finish_new(
                                    runtime.player, runtime.symbols, timeout_name.clone(), fallback.clone(), result.return_value.clone(),
                                ),
                            ChildCompletion::TimeoutForget { timeout_name } =>
                                super::handlers::datum_handlers::timeout::TimeoutDatumHandlers::finish_forget(
                                    runtime.player, runtime.symbols, timeout_name.clone(), result.return_value.clone(),
                                ),
                        }
                    });
                    match transformed {
                        Some(Ok(value)) => result.return_value = value,
                        Some(Err(error)) => return self.fail_current_frame(session, error),
                        None => {
                            self.phase = DriverPhase::Cancelled;
                            return DriverTurn::Error(super::cancelled_scope_error());
                        }
                    }
                }
                let delivered = Self::with_context(session, self.player_id, |ctx| {
                    if !parent_scope.validate_top(ctx.player) {
                        return Err(super::cancelled_scope_error());
                    }
                    validate_obj_call_result(ctx, &result.return_value)?;
                    if Self::policy_updates_last_handler_result(child_policy.as_ref()) {
                        ctx.player.last_handler_result = result.return_value.clone();
                    }
                    if Self::policy_is_global_scope(child_policy.as_ref()) {
                        let Some(scope) = ctx.player.scopes.get_mut(parent_scope.slot()) else {
                            return Err(super::cancelled_scope_error());
                        };
                        scope.return_value = result.return_value.clone();
                    }
                    if !super::deliver_scope_return(
                        ctx.player,
                        &parent_scope,
                        &result,
                        pending.push_return,
                    ) {
                        return Err(super::cancelled_scope_error());
                    }
                    Ok(())
                });
                match delivered {
                    Some(Ok(())) => DriverTurn::Waiting,
                    Some(Err(error)) => self.fail_current_frame(session, error),
                    None => {
                        self.phase = DriverPhase::Cancelled;
                        DriverTurn::Error(super::cancelled_scope_error())
                    }
                }
            }
            Ok(super::FrameSetup::Frame(frame)) => {
                self.frames.push(frame);
                self.child_return_policies.push(child_policy);
                self.phase = DriverPhase::Running;
                DriverTurn::Waiting
            }
            Ok(super::FrameSetup::Pending(plan)) => {
                let owner = self.owner.clone();
                let parent_anchor = plan.expectation.parent_scope();
                let ticket = match session.allocate_setup_action(&owner, parent_anchor.as_ref()) {
                    Some(ticket) => ticket,
                    None => return self.fail_current_frame(
                        session,
                        ScriptError::new("setup action ticket exhausted".to_owned()),
                    ),
                };
                let request = SetupCallbackRequest {
                    ticket: ticket.clone(),
                    player_id: plan.player_id,
                    owner: plan.owner,
                    kind: plan.kind,
                    script_ref: plan.script_ref,
                    handler_name: plan.handler_name,
                    receiver: plan.receiver,
                    args: plan.args,
                    retained_plan: plan.retained_plan,
                    expectation: plan.expectation,
                    child_policy: child_policy.clone(),
                    push_return: plan.push_return,
                };
                self.setup_parent_scope = parent_anchor;
                self.setup_child_policy = child_policy;
                self.setup_expectation = Some(request.expectation.clone());
                self.setup_push_return = request.push_return;
                self.phase = DriverPhase::Awaiting {
                    ticket,
                    resume: ResumePhase::SetupCallback,
                };
                DriverTurn::Pending(PendingAction::Setup(request))
            }
            Err(error) => {
                if Self::policy_has_ext_call_layer(child_policy.as_ref()) {
                    self.release_ext_call_layer(session);
                }
                self.fail_current_frame(session, error)
            }
        }
    }

    fn fail_current_frame(&mut self, session: &mut RuntimeSession, error: ScriptError) -> DriverTurn {
        self.release_static_event_guard(session);
        // sendSprite/sendAllSprites and call(list, ...) are broadcasts.  A
        // non-Abort child error is a warning in Director: continue with the
        // next receiver (and eventually static fallback) while Abort stops
        // the whole broadcast.  Convert the failed child into a passed,
        // Void frame so the ordinary teardown/aggregation path remains the
        // single owner of sequencing and return delivery.
        if error.code != super::ScriptErrorCode::Abort
            && matches!(
                self.child_return_policies.last(),
                Some(Some(ChildReturnPolicy::Broadcast { continue_on_error: true, .. }))
            )
        {
            warn!("broadcast child handler failed; continuing: {}", error.message);
            let marked = self
                .current_context()
                .and_then(|frame| {
                    Self::with_context(session, self.player_id, |runtime| {
                        if !frame.scope.validate_top(runtime.player) {
                            return false;
                        }
                        let Some(scope) = runtime.player.scopes.get_mut(frame.scope.slot()) else {
                            return false;
                        };
                        scope.return_value = DatumRef::Void;
                        scope.passed = true;
                        true
                    })
                })
                .unwrap_or(false);
            if marked {
                return self.finish_current_frame(session);
            }
        }
        self.release_all_ext_call_layers(session);
        let valid = self.frames.last().is_some_and(|frame| {
            Self::with_context(session, self.player_id, |ctx| {
                frame.ctx.scope.validate_top(ctx.player)
            })
            .unwrap_or(false)
        });
        if !valid {
            self.phase = DriverPhase::Cancelled;
            return DriverTurn::Error(super::cancelled_scope_error());
        }
        self.phase = DriverPhase::Failed(error.clone());
        DriverTurn::Error(error)
    }

    fn apply_setup_resume(&mut self, session: &mut RuntimeSession) -> DriverTurn {
        let DriverPhase::Resuming { completion, .. } =
            std::mem::replace(&mut self.phase, DriverPhase::Running)
        else {
            return DriverTurn::Error(super::cancelled_scope_error());
        };
        let child_policy = self.setup_child_policy.take();
        self.release_static_event_guard(session);
        let expectation = self.setup_expectation.take();
        let release_layer = Self::policy_has_ext_call_layer(child_policy.as_ref());
        let mut result = match completion {
            ActionCompletion::InternalResult(result) => result,
            ActionCompletion::InternalError(error) | ActionCompletion::Error(error) => {
                if matches!(
                    child_policy.as_ref(),
                    Some(ChildReturnPolicy::Broadcast { continue_on_error: true, .. })
                ) {
                    warn!("broadcast setup callback failed; continuing: {}", error.message);
                    DatumRef::Void
                } else {
                    if release_layer {
                        self.release_ext_call_layer(session);
                    }
                    self.phase = DriverPhase::Failed(error.clone());
                    return DriverTurn::Error(error);
                }
            }
            ActionCompletion::Resume => {
                if release_layer {
                    self.release_ext_call_layer(session);
                }
                self.phase = DriverPhase::Cancelled;
                return DriverTurn::Error(super::cancelled_scope_error());
            }
            ActionCompletion::RetryInternal(_) => {
                if release_layer {
                    self.release_ext_call_layer(session);
                }
                self.phase = DriverPhase::Cancelled;
                return DriverTurn::Error(ScriptError::new(
                    "invalid setup callback retry".to_owned(),
                ));
            }
        };

        let valid = Self::with_context(session, self.player_id, |ctx| {
            self.owner.same_identity(&ctx.player.owner)
                && self.owner.is_arena_live()
                && expectation
                    .as_ref()
                    .is_some_and(|expectation| expectation.validate(ctx.player))
                && validate_obj_call_result(ctx, &result).is_ok()
        })
        .unwrap_or(false);
        if !valid {
            if release_layer {
                self.release_ext_call_layer(session);
            }
            self.phase = DriverPhase::Cancelled;
            return DriverTurn::Error(super::cancelled_scope_error());
        }

        // A setup callback can stand in for one member of an ancestor chain.
        // Continue that chain exactly as the synchronous early path does,
        // without advancing the already-advanced parent a second time.
        if let Some(ChildReturnPolicy::Ancestor {
            remaining,
            final_push_return,
            ext_call_layer,
            delivery,
        }) = child_policy.clone()
        {
            let mut remaining_calls = remaining.into_iter();
            while let Some(call) = remaining_calls.next() {
                match self.resolve_ancestor_pending(session, &call) {
                    Ok(Some(mut pending)) => {
                        let remaining: Vec<_> = remaining_calls.collect();
                        pending.push_return = remaining.is_empty() && final_push_return;
                        return self.start_child_without_advancing(
                            session,
                            pending,
                            Some(ChildReturnPolicy::Ancestor {
                                remaining,
                                final_push_return,
                                ext_call_layer,
                                delivery,
                            }),
                        );
                    }
                    Ok(None) => {}
                    Err(error) => return self.fail_current_frame(session, error),
                }
            }
        }

        if let Some(ChildReturnPolicy::Broadcast {
            remaining,
            fallback,
            final_push_return,
            ext_call_layer,
            delivery,
            mut last_return_value,
            handled: _previously_handled,
            continue_on_error,
        }) = child_policy.clone()
        {
            // Setup callbacks are already classified as handled by the
            // virtual/JS boundary.  A successful Void result still stops
            // broadcast fallback just like a bytecode handler that passes
            // no value.
            let handled = true;
            if result != DatumRef::Void {
                last_return_value = result.clone();
            }
            let mut remaining_calls = remaining.into_iter();
            while let Some(call) = remaining_calls.next() {
                match self.resolve_broadcast_pending(session, &call) {
                    Ok(Some(mut pending)) => {
                        let remaining: Vec<_> = remaining_calls.collect();
                        pending.push_return = remaining.is_empty() && fallback.is_empty() && final_push_return;
                        return self.start_child_without_advancing(
                            session,
                            pending,
                            Some(ChildReturnPolicy::Broadcast {
                                remaining,
                                fallback: fallback.clone(),
                                final_push_return,
                                ext_call_layer,
                                delivery,
                                last_return_value,
                                handled,
                                continue_on_error,
                            }),
                        );
                    }
                    Ok(None) => {}
                    Err(error) => return self.fail_current_frame(session, error),
                }
            }
            if !handled {
                let mut remaining_static = fallback.into_iter();
                while let Some(static_call) = remaining_static.next() {
                    match self.resolve_static_pending(session, &static_call) {
                    Ok(Some(mut pending)) => {
                        let fallback: Vec<_> = remaining_static.collect();
                        pending.push_return = fallback.is_empty() && final_push_return;
                        return self.start_child_without_advancing(
                            session,
                            pending,
                            Some(ChildReturnPolicy::Broadcast {
                                remaining: Vec::new(),
                                fallback,
                                final_push_return,
                                ext_call_layer,
                                delivery,
                                last_return_value,
                                handled,
                                continue_on_error,
                            }),
                        );
                    }
                    Ok(None) => {}
                    Err(error) => return self.fail_current_frame(session, error),
                    }
                }
            }
            result = last_return_value;
        }

        if let Some(ChildReturnPolicy::Completion { completion, .. }) = &child_policy {
            let transformed = Self::with_context(session, self.player_id, |runtime| {
                match completion {
                    ChildCompletion::ConstructScript { fallback } =>
                        super::handlers::datum_handlers::script::ScriptDatumHandlers::finish_constructor(
                            runtime.player,
                            runtime.symbols,
                            fallback.clone(),
                            result.clone(),
                        ),
                    ChildCompletion::TimeoutNew { timeout_name, fallback } =>
                        super::handlers::datum_handlers::timeout::TimeoutDatumHandlers::finish_new(
                            runtime.player,
                            runtime.symbols,
                            timeout_name.clone(),
                            fallback.clone(),
                            result.clone(),
                        ),
                    ChildCompletion::TimeoutForget { timeout_name } =>
                        super::handlers::datum_handlers::timeout::TimeoutDatumHandlers::finish_forget(
                            runtime.player,
                            runtime.symbols,
                            timeout_name.clone(),
                            result.clone(),
                        ),
                }
            });
            match transformed {
                Some(Ok(value)) => result = value,
                Some(Err(error)) => return self.fail_current_frame(session, error),
                None => {
                    self.phase = DriverPhase::Cancelled;
                    return DriverTurn::Error(super::cancelled_scope_error());
                }
            }
        }

        if let Some(parent_scope) = self.setup_parent_scope.take() {
            let push_return = self.setup_push_return;
            let delivered = Self::with_context(session, self.player_id, |ctx| {
                if !parent_scope.validate_top(ctx.player) {
                    return Err(super::cancelled_scope_error());
                }
                validate_obj_call_result(ctx, &result)?;
                if Self::policy_updates_last_handler_result(child_policy.as_ref()) {
                    ctx.player.last_handler_result = result.clone();
                }
                if Self::policy_is_global_scope(child_policy.as_ref()) {
                    let Some(scope) = ctx.player.scopes.get_mut(parent_scope.slot()) else {
                        return Err(super::cancelled_scope_error());
                    };
                    scope.return_value = result.clone();
                }
                if !super::deliver_scope_return(ctx.player, &parent_scope, &ScopeResult {
                    return_value: result.clone(),
                    passed: false,
                }, push_return) {
                    return Err(super::cancelled_scope_error());
                }
                Ok(())
            });
            match delivered {
                Some(Ok(())) => {
                    if release_layer {
                        self.release_ext_call_layer(session);
                    }
                    self.phase = DriverPhase::Running;
                    return DriverTurn::Waiting;
                }
                Some(Err(error)) => return self.fail_current_frame(session, error),
                None => {
                    if release_layer {
                        self.release_ext_call_layer(session);
                    }
                    self.phase = DriverPhase::Cancelled;
                    return DriverTurn::Error(super::cancelled_scope_error());
                }
            }
        }

        if release_layer {
            self.release_ext_call_layer(session);
        }
        self.phase = DriverPhase::Completing;
        DriverTurn::Complete(ScopeResult {
            return_value: result,
            passed: false,
        })
    }

    pub(crate) fn complete(
        &mut self,
        session: &mut RuntimeSession,
        ticket: CompletionTicket,
        completion: ActionCompletion,
    ) -> bool {
        let (expected_ticket, expected_resume) = match &self.phase {
            DriverPhase::Awaiting { ticket, resume } => (ticket.clone(), *resume),
            _ => return false,
        };
        if expected_ticket.id != ticket.id
            || !Arc::ptr_eq(&expected_ticket.capability, &ticket.capability)
        {
            return false;
        }
        if self.frames.is_empty() && expected_resume == ResumePhase::SetupCallback {
            let valid = Self::with_context(session, self.player_id, |ctx| {
                self.owner.same_identity(&ctx.player.owner) && self.owner.is_arena_live()
            })
            .unwrap_or(false);
            if !valid {
                return false;
            }
            if let ActionCompletion::InternalResult(result) = &completion {
                let valid = Self::with_context(session, self.player_id, |ctx| {
                    validate_obj_call_result(ctx, result).is_ok()
                })
                .unwrap_or(false);
                if !valid {
                    return false;
                }
            }
            let Some((kind, resume)) = session.action_details(&ticket) else {
                return false;
            };
            if resume != expected_resume || !completion.matches_action(kind, resume) {
                return false;
            }
            if session
                .authorize_setup_action(&ticket, &self.owner, self.setup_parent_scope.as_ref())
                .is_none()
            {
                return false;
            }
            self.phase = DriverPhase::Resuming { resume, completion };
            return true;
        }
        let Some(frame) = self.frames.last() else { return false };
        let Some((owner, scope)) = Self::with_context(session, self.player_id, |ctx| {
            (ctx.player.owner.clone(), frame.ctx.scope.clone())
        }) else { return false };
        // Stored token equality is insufficient: reset/reuse can leave the
        // same locator in a different live scope. Validate the actual top
        // scope before looking up or consuming the action entry.
        let Some(live) = Self::with_context(session, self.player_id, |ctx| {
            scope.validate_top(ctx.player)
        }) else {
            return false;
        };
        if !live {
            return false;
        }
        let validate_completion = self
            .internal_effect
            .as_ref()
            .map(internal_completion_requires_validation)
            .unwrap_or(true);
        if validate_completion {
            if let ActionCompletion::InternalResult(result) = &completion {
            let Some(valid) = Self::with_context(session, self.player_id, |runtime| {
                validate_obj_call_result(runtime, result).is_ok()
            }) else {
                return false;
            };
            if !valid {
                return false;
            }
            }
        }
        let Some((kind, resume)) = session.action_details(&ticket) else {
            return false;
        };
        if resume != expected_resume || !completion.matches_action(kind, resume) {
            return false;
        }
        let Some((_kind, resume)) = session.authorize_action(
            &ticket,
            &owner,
            &scope,
            kind,
            resume,
        ) else {
            return false;
        };
        self.phase = DriverPhase::Resuming { resume, completion };
        true
    }
}

pub(crate) enum ActionCompletion {
    Resume,
    Error(ScriptError),
    InternalResult(DatumRef),
    InternalError(ScriptError),
    /// Re-enter the owning driver's normal internal-dispatch path after a
    /// host-side probe declined.  The request retains its one-shot bypass
    /// marker; the original opcode effect remains on the continuation.
    RetryInternal(InternalVmRequest),
}

impl ActionCompletion {
    fn matches_action(&self, kind: ActionKind, resume: ResumePhase) -> bool {
        match self {
            Self::Resume => matches!(
                (kind, resume),
                (ActionKind::Breakpoint, ResumePhase::AfterBreakpoint)
                    | (ActionKind::Breakpoint, ResumePhase::AfterStep)
                    | (ActionKind::CooperativeYield, ResumePhase::ApplyOpcode)
                    | (ActionKind::SetupCallback, ResumePhase::SetupCallback)
                    | (ActionKind::Trace, ResumePhase::SetupCallback)
                    | (ActionKind::Trace, ResumePhase::Teardown)
            ),
            Self::Error(_) => {
                matches!((kind, resume), (ActionKind::ErrorPause, ResumePhase::ErrorUnwind))
            }
            Self::InternalResult(_) => matches!(
                (kind, resume),
                (ActionKind::InternalInvocation, ResumePhase::ApplyOpcode)
                    | (ActionKind::SetupCallback, ResumePhase::SetupCallback)
            ),
            Self::InternalError(_) => matches!(
                (kind, resume),
                (ActionKind::InternalInvocation, ResumePhase::ApplyOpcode)
                    | (ActionKind::SetupCallback, ResumePhase::SetupCallback)
            ),
            Self::RetryInternal(_) => {
                matches!((kind, resume), (ActionKind::InternalInvocation, ResumePhase::ApplyOpcode))
            }
        }
    }
}

fn validate_obj_call_result(
    runtime: &ExecutionContext<'_>,
    result: &DatumRef,
) -> Result<(), ScriptError> {
    if result
        .owner()
        .is_some_and(|result_owner| !result_owner.same_identity(&runtime.player.owner))
    {
        return Err(ScriptError::new_code(
            super::ScriptErrorCode::InvalidReference,
            "foreign datum result".to_owned(),
        ));
    }
    checked_internal_datum(runtime.player, runtime.symbols, result).map(|_| ())
}

fn checked_context_name(
    ctx: &BytecodeHandlerContext,
    name_id: u16,
) -> Result<Symbol, ScriptError> {
    ctx.code
        .names
        .get(name_id as usize)
        .cloned()
        .ok_or_else(|| {
            ScriptError::new_code(
                super::ScriptErrorCode::InvalidReference,
                format!("opcode name index {} is out of range", name_id),
            )
        })
}

fn opcode_name_for_error(opcode: OpCode) -> &'static str {
    match opcode {
        OpCode::NewObj => "new_obj",
        OpCode::ExtCall => "ext_call",
        OpCode::ObjCallV4 => "obj_call_v4",
        OpCode::SetObjProp => "set_obj_prop",
        OpCode::TellCall => "tell_call",
        _ => "internal opcode",
    }
}

fn pop_internal_ref(
    runtime: &mut ExecutionContext<'_>,
    ctx: &BytecodeHandlerContext,
    label: &str,
) -> Result<DatumRef, ScriptError> {
    let (scopes, allocator, bitmap_manager) = (
        &mut runtime.player.scopes,
        &mut runtime.player.allocator,
        &mut runtime.player.bitmap_manager,
    );
    scopes
        .get_mut(ctx.scope_ref())
        .ok_or_else(super::cancelled_scope_error)?
        .stack
        .pop_ref_with(allocator, bitmap_manager)
        .ok_or_else(|| ScriptError::new(format!("{}: stack underflow", label)))
}

fn pop_internal_call_args(
    runtime: &mut ExecutionContext<'_>,
    ctx: &BytecodeHandlerContext,
    opcode: OpCode,
) -> Result<(Vec<DatumRef>, bool), ScriptError> {
    let (scopes, allocator, bitmap_manager) = (
        &mut runtime.player.scopes,
        &mut runtime.player.allocator,
        &mut runtime.player.bitmap_manager,
    );
    scopes
        .get_mut(ctx.scope_ref())
        .ok_or_else(super::cancelled_scope_error)?
        .pop_call_args(allocator, bitmap_manager)
        .ok_or_else(|| {
            ScriptError::new(format!(
                "{}: expected arg marker on stack",
                opcode_name_for_error(opcode)
            ))
        })
}

pub(crate) fn checked_internal_datum<'a>(
    player: &'a super::DirPlayer,
    symbols: &super::symbols::symbol_table::SymbolTable,
    datum_ref: &DatumRef,
) -> Result<&'a Datum, ScriptError> {
    let datum = match datum_ref {
        DatumRef::Void => &Datum::Void,
        _ => player
            .allocator
            .try_get_datum(datum_ref)
            .ok_or_else(|| {
                ScriptError::new_code(
                    super::ScriptErrorCode::InvalidReference,
                    format!("invalid datum reference {datum_ref}"),
                )
            })?,
    };
    validate_owned_datum_graph(player, symbols, datum_ref)?;
    Ok(datum)
}

/// Validate all owner-bound handles retained by one datum graph.
///
/// This runs at transfer and deferred-access boundaries, rather than during
/// every allocation. Each child reference is checked against the allocator
/// before the visited set is consulted, so a colliding numeric ID cannot make
/// a foreign or stale handle appear valid. The worklist keeps deeply cyclic
/// Lingo lists off the Rust call stack; script-instance property graphs remain
/// opaque and are validated when their handles are reached.
pub(crate) fn validate_owned_datum_graph(
    player: &super::DirPlayer,
    symbols: &super::symbols::symbol_table::SymbolTable,
    root: &DatumRef,
) -> Result<(), ScriptError> {
    let root_datum = match root {
        DatumRef::Void => return Ok(()),
        _ => player
            .allocator
            .try_get_datum(root)
            .ok_or_else(|| {
                ScriptError::new_code(
                    super::ScriptErrorCode::InvalidReference,
                    format!("invalid datum reference {root}"),
                )
            })?,
    };
    super::compare::validate_direct_symbol_fields(root_datum, symbols)?;
    if !datum_has_owned_children(root_datum) {
        validate_owned_datum_leaf(player, root_datum)?;
        return Ok(());
    }

    let mut pending = vec![root];
    let mut visited = HashSet::new();
    while let Some(reference) = pending.pop() {
        let datum = match reference {
            DatumRef::Void => continue,
            _ => player
                .allocator
                .try_get_datum(reference)
                .ok_or_else(|| {
                    ScriptError::new_code(
                        super::ScriptErrorCode::InvalidReference,
                        format!("invalid datum reference {reference}"),
                    )
                })?,
        };
        let id = reference.unwrap();
        if !visited.insert(id) {
            continue;
        }

        super::compare::validate_direct_symbol_fields(datum, symbols)?;
        validate_owned_datum_leaf(player, datum)?;
        match datum {
            Datum::List(_, items, _) => pending.extend(items.iter()),
            Datum::PropList(entries, _) => {
                for (key, value) in entries {
                    pending.push(key);
                    pending.push(value);
                }
            }
            Datum::StringChunk(
                crate::director::lingo::datum::StringChunkSource::Datum(child),
                _,
                _,
            ) => pending.push(child),
            Datum::TimeoutInstance(data) => {
                pending.push(&data.callback);
                pending.push(&data.target);
                if let Some(script_instance) = &data.script_instance {
                    pending.push(script_instance);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn datum_has_owned_children(datum: &Datum) -> bool {
    matches!(
        datum,
        Datum::List(..)
            | Datum::PropList(..)
            | Datum::StringChunk(
                crate::director::lingo::datum::StringChunkSource::Datum(..),
                _,
                _,
            )
            | Datum::TimeoutInstance(..)
    )
}

fn validate_owned_datum_leaf(
    player: &super::DirPlayer,
    datum: &Datum,
) -> Result<(), ScriptError> {
    match datum {
        Datum::ScriptInstanceRef(instance_ref)
        | Datum::VarRef(VarRef::ScriptInstance(instance_ref)) => {
            if !instance_ref.owner().same_identity(&player.owner)
                || player
                    .allocator
                    .get_script_instance_opt(instance_ref)
                    .is_none()
            {
                return Err(ScriptError::new_code(
                    super::ScriptErrorCode::InvalidReference,
                    "foreign or stale ScriptInstanceRef".to_owned(),
                ));
            }
        }
        Datum::BitmapRef(handle) => {
            if player.bitmap_manager.get_bitmap_handle(handle).is_none() {
                return Err(ScriptError::new_code(
                    super::ScriptErrorCode::InvalidReference,
                    "foreign or stale BitmapHandle".to_owned(),
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

fn global_handler_exists(
    player: &super::DirPlayer,
    symbols: &super::symbols::symbol_table::SymbolTable,
    handler_name: &Symbol,
) -> Result<bool, ScriptError> {
    symbols.display(handler_name).map_err(|_| {
        ScriptError::new_code(
            super::ScriptErrorCode::InvalidReference,
            "foreign or stale handler symbol".to_owned(),
        )
    })?;
    Ok(active_static_script_refs(player, symbols)?.iter().any(|script_ref| {
        player
            .movie
            .cast_manager
            .get_script_by_ref(script_ref)
            .and_then(|script| script.get_own_handler_ref(handler_name.clone()))
            .is_some()
    }))
}

fn active_static_script_refs(
    player: &super::DirPlayer,
    symbols: &super::symbols::symbol_table::SymbolTable,
) -> Result<Vec<CastMemberRef>, ScriptError> {
    let movie = &player.movie;
    let frame_script = movie.score.get_script_in_frame(movie.current_frame);
    let movie_scripts_cache = movie.cast_manager.get_movie_scripts();
    let movie_scripts = movie_scripts_cache.as_ref().ok_or_else(|| {
        ScriptError::new_code(
            super::ScriptErrorCode::InvalidReference,
            "active movie scripts are unavailable".to_owned(),
        )
    })?;
    let mut active_script_refs: Vec<CastMemberRef> = Vec::new();
    for script in movie_scripts.iter() {
        active_script_refs.push(script.member_ref.clone());
    }
    if let Some(frame_script) = frame_script {
        active_script_refs.push(CastMemberRef {
            cast_lib: frame_script.cast_lib.into(),
            cast_member: frame_script.cast_member.into(),
        });
    }
    for global_ref in player.globals.values() {
        if let Datum::VarRef(VarRef::Script(script_ref)) =
            checked_internal_datum(player, symbols, global_ref)?
        {
            active_script_refs.push(script_ref.clone());
        }
    }
    Ok(active_script_refs)
}

fn internal_completion_requires_validation(effect: &InternalResultEffect) -> bool {
    match effect {
        InternalResultEffect::SetProperty => false,
        InternalResultEffect::Global {
            return_value: Some(_),
            ..
        } => false,
        InternalResultEffect::Tell {
            target: Some(_),
            push_return: false,
            return_value: None,
            ..
        } => false,
        InternalResultEffect::Tell {
            target: None,
            return_value: Some(_),
            ..
        } => false,
        _ => true,
    }
}

struct ActionEntry {
    owner: OwnerToken,
    scope: Option<super::ScopeToken>,
    kind: ActionKind,
    resume: ResumePhase,
    capability: Arc<ActionCapability>,
}

pub(crate) struct ActionRegistry {
    next: u64,
    pending: HashMap<ActionId, ActionEntry>,
}

impl ActionRegistry {
    pub(crate) fn new() -> Self {
        Self { next: 0, pending: HashMap::new() }
    }

    pub(crate) fn allocate(
        &mut self,
        owner: &OwnerToken,
        scope: Option<&super::ScopeToken>,
        kind: ActionKind,
        resume: ResumePhase,
    ) -> Option<CompletionTicket> {
        self.next = self.next.checked_add(1)?;
        let id = ActionId(self.next);
        let capability = Arc::new(ActionCapability);
        self.pending.insert(id, ActionEntry {
            owner: owner.clone(),
            scope: scope.cloned(),
            kind,
            resume,
            capability: capability.clone(),
        });
        Some(CompletionTicket { id, capability })
    }

    pub(crate) fn player_id(&self, ticket: &CompletionTicket) -> Option<PlayerId> {
        self.pending
            .get(&ticket.id)
            .map(|entry| entry.owner.key().player as PlayerId)
    }

    pub(crate) fn details(&self, ticket: &CompletionTicket) -> Option<(ActionKind, ResumePhase)> {
        let entry = self.pending.get(&ticket.id)?;
        if !Arc::ptr_eq(&entry.capability, &ticket.capability) {
            return None;
        }
        Some((entry.kind, entry.resume))
    }

    pub(crate) fn validate(
        &self,
        ticket: &CompletionTicket,
        owner: &OwnerToken,
        scope: Option<&super::ScopeToken>,
        expected_kind: ActionKind,
        expected_resume: ResumePhase,
    ) -> bool {
        let Some(entry) = self.pending.get(&ticket.id) else {
            return false;
        };
        entry.kind == expected_kind
            && entry.resume == expected_resume
            && Arc::ptr_eq(&entry.capability, &ticket.capability)
            && entry.owner.same_identity(owner)
            && match (&entry.scope, scope) {
                (Some(entry_scope), Some(scope)) => entry_scope.same_identity(scope),
                (None, None) => true,
                _ => false,
            }
            && entry.owner.is_arena_live()
    }

    pub(crate) fn authorize(
        &mut self,
        ticket: &CompletionTicket,
        owner: &OwnerToken,
        scope: Option<&super::ScopeToken>,
        expected_kind: ActionKind,
        expected_resume: ResumePhase,
    ) -> Option<(ActionKind, ResumePhase)> {
        let entry = self.pending.get(&ticket.id)?;
        if entry.kind != expected_kind
            || entry.resume != expected_resume
            || !Arc::ptr_eq(&entry.capability, &ticket.capability)
            || !entry.owner.same_identity(owner)
            || match (&entry.scope, scope) {
                (Some(entry_scope), Some(scope)) => !entry_scope.same_identity(scope),
                (None, None) => false,
                _ => true,
            }
            || !entry.owner.is_arena_live()
        {
            return None;
        }
        let entry = self.pending.remove(&ticket.id)?;
        Some((entry.kind, entry.resume))
    }

    pub(crate) fn cancel_owner(&mut self, owner: &OwnerToken) {
        self.pending
            .retain(|_, entry| !entry.owner.same_identity(owner));
    }

    /// Retire one callback capability when its awaiting owner future is
    /// dropped. The identity check keeps a stale callback from cancelling a
    /// replacement action that happens to reuse the numeric player id.
    pub(crate) fn cancel_ticket(&mut self, ticket: &CompletionTicket) {
        let Some(entry) = self.pending.get(&ticket.id) else {
            return;
        };
        if Arc::ptr_eq(&entry.capability, &ticket.capability) {
            self.pending.remove(&ticket.id);
        }
    }
}

impl super::ScopeToken {
    fn same_identity(&self, other: &Self) -> bool {
        self.owner.same_identity(&other.owner)
            && self.slot == other.slot
            && self.generation == other.generation
            && self.epoch == other.epoch
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use async_std::channel;
    use crate::director::chunks::{handler::{Bytecode, HandlerDef}, script::ScriptChunk};
    use crate::director::enums::ScriptType;
    use crate::director::lingo::datum::Datum;
    use crate::director::lingo::opcode::OpCode;
    use crate::player::allocator::ScriptInstanceAllocatorTrait;
    use crate::player::cast_lib::{CastLib, CastMemberRef};
    use crate::player::handlers::datum_handlers::string::StringDatumUtils;
    use crate::player::handlers::string::StringHandlers;
    use crate::player::script::Script;
    use crate::player::score::SpriteChannel;
    use crate::player::ScriptErrorCode;
    use crate::player::scope::StackDatum;
    use crate::player::symbols::{builtin::BuiltInSymbol, symbol::Symbol, symbol_table::SymbolOwner};
    use crate::player::virtual_scripts::{VirtualScriptHandler, VirtualScriptRegistry};
    use std::{cell::RefCell, collections::{HashMap, VecDeque}, rc::Rc, sync::Arc};

    struct VirtualNew;

    impl VirtualScriptHandler for VirtualNew {
        fn script_type(&self) -> ScriptType {
            ScriptType::Movie
        }

        fn call_handler(
            &self,
            player: &mut super::super::DirPlayer,
            _symbols: &super::super::symbols::symbol_table::SymbolTable,
            _instance: Option<&super::super::script_ref::ScriptInstanceRef>,
            name: Symbol,
            _args: &Vec<DatumRef>,
        ) -> Result<Option<DatumRef>, ScriptError> {
            if name == Symbol::builtin(BuiltInSymbol::New) {
                Ok(Some(player.alloc_datum(Datum::Int(17))))
            } else {
                Ok(None)
            }
        }
    }

    struct ResetAndDecline;

    impl VirtualScriptHandler for ResetAndDecline {
        fn script_type(&self) -> ScriptType {
            ScriptType::Movie
        }

        fn call_handler(
            &self,
            player: &mut super::super::DirPlayer,
            _symbols: &super::super::symbols::symbol_table::SymbolTable,
            _instance: Option<&super::super::script_ref::ScriptInstanceRef>,
            _name: Symbol,
            _args: &Vec<DatumRef>,
        ) -> Result<Option<DatumRef>, ScriptError> {
            player.owner.begin_reset();
            Ok(None)
        }
    }

    struct EarlyChild;

    impl VirtualScriptHandler for EarlyChild {
        fn call_handler(
            &self,
            player: &mut super::super::DirPlayer,
            symbols: &super::super::symbols::symbol_table::SymbolTable,
            _instance: Option<&super::super::script_ref::ScriptInstanceRef>,
            name: Symbol,
            _args: &Vec<DatumRef>,
        ) -> Result<Option<DatumRef>, ScriptError> {
            if symbols.display(&name) == Ok("child") {
                Ok(Some(player.alloc_datum(Datum::Int(31))))
            } else {
                Ok(None)
            }
        }
    }

    fn prepared_running_driver(bytecode_array: Vec<Bytecode>) -> (RuntimeSession, DriverContinuation) {
        let mut session = RuntimeSession::new(SymbolOwner { session: 7, generation: 1 });
        let (tx, _rx) = channel::unbounded();
        assert!(session.add_player(1, tx));
        let member_ref = CastMemberRef { cast_lib: 1, cast_member: 1 };
        let handler_name = session.symbols_mut().intern("driverTest");
        let handler = Rc::new(HandlerDef {
            name_id: 0,
            bytecode_array,
            bytecode_index_map: fxhash::FxHashMap::default(),
            argument_name_ids: vec![],
            local_name_ids: vec![],
            global_name_ids: vec![],
            compiled_ir: RefCell::new(None),
        });
        let child_handler_name = session.symbols_mut().intern("child");
        let child_handler = Rc::new(HandlerDef {
            name_id: 1,
            bytecode_array: vec![Bytecode::new(OpCode::Ret, 0, 0)],
            bytecode_index_map: fxhash::FxHashMap::default(),
            argument_name_ids: vec![],
            local_name_ids: vec![],
            global_name_ids: vec![],
            compiled_ir: RefCell::new(None),
        });
        let mut handlers = fxhash::FxHashMap::default();
        handlers.insert(handler_name.clone(), handler);
        handlers.insert(child_handler_name.clone(), child_handler);
        let script = Rc::new(Script {
            member_ref: member_ref.clone(),
            name: "driver-test".to_owned(),
            chunk: ScriptChunk {
                script_number: 1,
                literals: vec![],
                handlers: vec![],
                property_name_ids: vec![],
                property_defaults: HashMap::new(),
            },
            script_type: ScriptType::Movie,
            handlers,
            handler_names_raw: vec!["driverTest".to_owned(), "child".to_owned()],
            handler_names: vec![handler_name.clone(), child_handler_name.clone()],
            properties: RefCell::new(fxhash::FxHashMap::default()),
        });
        let append_name = session.symbols_mut().intern("append");
        let count_name = session.symbols_mut().intern("count");
        let unsupported_name = session.symbols_mut().intern("unsupportedCall");
        let get_at_name = session.symbols_mut().intern("getAt");
        let set_at_name = session.symbols_mut().intern("setAt");
        let distance_to_name = session.symbols_mut().intern("distanceTo");
        let hex_string_name = session.symbols_mut().intern("hexString");
        let sin_name = session.symbols_mut().intern("sin");
        let delete_name = session.symbols_mut().intern("delete");
        let script_name = Symbol::builtin(BuiltInSymbol::Script);
        let return_name = Symbol::builtin(BuiltInSymbol::Return);
        let nothing_name = Symbol::builtin(BuiltInSymbol::Nothing);
        let voidp_name = Symbol::builtin(BuiltInSymbol::Voidp);
        session.with_player(1, |ctx| {
            let mut cast = CastLib::test_external(1, 0);
            cast.name_symbols = Rc::from(vec![
                handler_name.clone(),
                child_handler_name.clone(),
                append_name.clone(),
                count_name.clone(),
                unsupported_name.clone(),
                get_at_name,
                set_at_name,
                distance_to_name,
                hex_string_name,
                sin_name,
                delete_name,
                script_name,
                return_name,
                nothing_name,
                voidp_name,
            ]);
            cast.scripts.insert(1, script);
            ctx.player.movie.cast_manager.casts.push(cast);
        }).unwrap();
        let expectation = session.with_player(1, |ctx| {
            super::super::SetupExpectation::capture(ctx.player, None)
        }).unwrap();
        let frame = match super::super::setup_handler_frame(
            &mut session,
            1,
            None,
            (member_ref, handler_name),
            &Vec::new(),
            true,
            false,
            expectation,
        ).unwrap() {
            super::super::FrameSetup::Frame(frame) => frame,
            super::super::FrameSetup::Early(_) => panic!("test handler unexpectedly answered early"),
            super::super::FrameSetup::Pending(_) => panic!("test handler unexpectedly suspended"),
        };
        let owner = session.with_player(1, |ctx| ctx.player.owner.clone()).unwrap();
        let driver = DriverContinuation {
            player_id: 1,
            owner,
            phase: DriverPhase::Running,
            frames: vec![frame],
            backjumps: 0,
            total_backjumps: 0,
            last_yield_ms: 0,
            obj_call_push_return: None,
            child_return_policies: vec![None],
            internal_effect: None,
            ext_call_layers: Vec::new(),
            internal_ext_call_layer: false,
            static_event_guard: None,
            step_callback: false,
            setup_pending: None,
            setup_child_policy: None,
            setup_parent_scope: None,
            setup_push_return: false,
            setup_expectation: None,
        };
        (session, driver)
    }

    #[test]
    fn checked_internal_datum_rejects_nested_foreign_and_stale_script_refs() {
        let mut session = RuntimeSession::new(SymbolOwner { session: 7, generation: 1 });
        assert!(session.add_player(1, channel::unbounded().0));
        assert!(session.add_player(2, channel::unbounded().0));

        let local_instance = session
            .with_player(1, |ctx| {
                ctx.player.allocator.alloc_script_instance(crate::player::script::ScriptInstance {
                    instance_id: 7,
                    script: CastMemberRef { cast_lib: 1, cast_member: 1 },
                    ancestor: None,
                    properties: fxhash::FxHashMap::default(),
                    begin_sprite_called: false,
                })
            })
            .unwrap();
        let foreign_instance = session
            .with_player(2, |ctx| {
                ctx.player.allocator.alloc_script_instance(crate::player::script::ScriptInstance {
                    instance_id: 7,
                    script: CastMemberRef { cast_lib: 1, cast_member: 1 },
                    ancestor: None,
                    properties: fxhash::FxHashMap::default(),
                    begin_sprite_called: false,
                })
            })
            .unwrap();
        assert_eq!(local_instance.id(), foreign_instance.id());
        let foreign_datum = session
            .with_player(2, |ctx| ctx.player.alloc_datum(Datum::Int(1)))
            .unwrap();

        let (local_prop, foreign_cases, collision_prop, cycle, local_collision) = session
            .with_player(1, |ctx| {
                let local_item = ctx
                    .player
                    .alloc_datum(Datum::ScriptInstanceRef(local_instance.clone()));
                let local_list = ctx.player.alloc_datum(Datum::List(
                    crate::director::lingo::datum::DatumType::List,
                    VecDeque::from([local_item]),
                    false,
                ));
                let local_key = ctx.player.alloc_datum(Datum::String("local".to_owned()));
                let local_prop = ctx.player.alloc_datum(Datum::PropList(
                    VecDeque::from([(local_key, local_list)]),
                    false,
                ));

                let foreign_item = ctx
                    .player
                    .alloc_datum(Datum::ScriptInstanceRef(foreign_instance.clone()));
                let foreign_list = ctx.player.alloc_datum(Datum::List(
                    crate::director::lingo::datum::DatumType::List,
                    VecDeque::from([foreign_item.clone()]),
                    false,
                ));
                let foreign_key = ctx.player.alloc_datum(Datum::String("foreign".to_owned()));
                let foreign_prop = ctx.player.alloc_datum(Datum::PropList(
                    VecDeque::from([(foreign_key, foreign_list)]),
                    false,
                ));
                let foreign_chunk = ctx.player.alloc_datum(Datum::StringChunk(
                    crate::director::lingo::datum::StringChunkSource::Datum(foreign_item.clone()),
                    crate::director::lingo::datum::StringChunkExpr {
                        chunk_type: crate::director::lingo::datum::StringChunkType::Char,
                        start: 1,
                        end: 1,
                        item_delimiter: ',',
                    },
                    "foreign".to_owned(),
                ));
                let foreign_timeout = ctx.player.alloc_datum(Datum::TimeoutInstance(Box::new(
                    crate::director::lingo::datum::TimeoutInstanceData {
                        name: "foreign".to_owned(),
                        duration: 1,
                        callback: foreign_item.clone(),
                        target: DatumRef::Void,
                        script_instance: Some(foreign_item.clone()),
                    },
                )));
                let foreign_var = ctx.player.alloc_datum(Datum::VarRef(
                    VarRef::ScriptInstance(foreign_instance.clone()),
                ));
                let local_collision = ctx.player.alloc_datum(Datum::Int(1));
                let collision_key = ctx.player.alloc_datum(Datum::String("collision".to_owned()));
                let collision_list = ctx.player.alloc_datum(Datum::List(
                    crate::director::lingo::datum::DatumType::List,
                    // The validator's worklist is LIFO: visit the local ID
                    // first, then prove the foreign same-ID child is checked
                    // before the visited set can suppress it.
                    VecDeque::from([foreign_datum.clone(), local_collision.clone()]),
                    false,
                ));
                let collision_prop = ctx.player.alloc_datum(Datum::PropList(
                    VecDeque::from([(collision_key, collision_list)]),
                    false,
                ));

                let cycle = ctx.player.alloc_datum(Datum::List(
                    crate::director::lingo::datum::DatumType::List,
                    VecDeque::new(),
                    false,
                ));
                if let Datum::List(_, items, _) = ctx.player.get_datum_mut(&cycle) {
                    items.push_back(cycle.clone());
                } else {
                    panic!("cycle fixture was not a list");
                }
                (
                    local_prop,
                    vec![foreign_prop, foreign_chunk, foreign_timeout, foreign_var],
                    collision_prop,
                    cycle,
                    local_collision,
                )
            })
            .unwrap();
        assert_eq!(local_collision.unwrap(), foreign_datum.unwrap());

        let local_result = session
            .with_player(1, |ctx| {
                super::checked_internal_datum(ctx.player, ctx.symbols, &local_prop).map(|_| ())
            })
        .unwrap();
        assert!(local_result.is_ok());

        for foreign_case in &foreign_cases {
            let foreign_result = session
                .with_player(1, |ctx| {
                    super::checked_internal_datum(ctx.player, ctx.symbols, foreign_case)
                        .map(|_| ())
                })
                .unwrap();
            assert_eq!(
                foreign_result.err().map(|error| error.code),
                Some(ScriptErrorCode::InvalidReference)
            );
        }

        let collision_result = session
            .with_player(1, |ctx| {
                super::checked_internal_datum(ctx.player, ctx.symbols, &collision_prop)
                    .map(|_| ())
            })
            .unwrap();
        assert_eq!(
            collision_result.err().map(|error| error.code),
            Some(ScriptErrorCode::InvalidReference)
        );

        let cycle_result = session
            .with_player(1, |ctx| {
                super::checked_internal_datum(ctx.player, ctx.symbols, &cycle).map(|_| ())
            })
        .unwrap();
        assert!(cycle_result.is_ok());

        let old_local_owner = session.with_player(1, |ctx| ctx.player.owner.clone()).unwrap();
        session.reset_player_owned(1, &old_local_owner).unwrap();
        let (stale_local_prop, fresh_local_prop, fresh_instance) = session
            .with_player(1, |ctx| {
                let stale_item = ctx
                    .player
                    .alloc_datum(Datum::ScriptInstanceRef(local_instance.clone()));
                let stale_list = ctx.player.alloc_datum(Datum::List(
                    crate::director::lingo::datum::DatumType::List,
                    VecDeque::from([stale_item]),
                    false,
                ));
                let stale_key = ctx.player.alloc_datum(Datum::String("stale".to_owned()));
                let stale_local_prop = ctx.player.alloc_datum(Datum::PropList(
                    VecDeque::from([(stale_key, stale_list)]),
                    false,
                ));

                let fresh_instance = ctx.player.allocator.alloc_script_instance(
                    crate::player::script::ScriptInstance {
                        instance_id: local_instance.id(),
                        script: CastMemberRef { cast_lib: 1, cast_member: 1 },
                        ancestor: None,
                        properties: fxhash::FxHashMap::default(),
                        begin_sprite_called: false,
                    },
                );
                let fresh_item = ctx
                    .player
                    .alloc_datum(Datum::ScriptInstanceRef(fresh_instance.clone()));
                let fresh_list = ctx.player.alloc_datum(Datum::List(
                    crate::director::lingo::datum::DatumType::List,
                    VecDeque::from([fresh_item]),
                    false,
                ));
                let fresh_key = ctx.player.alloc_datum(Datum::String("fresh".to_owned()));
                let fresh_local_prop = ctx.player.alloc_datum(Datum::PropList(
                    VecDeque::from([(fresh_key, fresh_list)]),
                    false,
                ));
                (stale_local_prop, fresh_local_prop, fresh_instance)
            })
            .unwrap();
        assert_eq!(fresh_instance.id(), local_instance.id());

        let stale_result = session
            .with_player(1, |ctx| {
                super::checked_internal_datum(ctx.player, ctx.symbols, &stale_local_prop)
                    .map(|_| ())
            })
            .unwrap();
        assert_eq!(
            stale_result.err().map(|error| error.code),
            Some(ScriptErrorCode::InvalidReference)
        );
        let fresh_result = session
            .with_player(1, |ctx| {
                super::checked_internal_datum(ctx.player, ctx.symbols, &fresh_local_prop)
                    .map(|_| ())
            })
            .unwrap();
        assert!(fresh_result.is_ok());
    }

    fn prepared_driver() -> (RuntimeSession, DriverContinuation, CompletionTicket) {
        let (mut session, mut driver) =
            prepared_running_driver(vec![Bytecode::new(OpCode::Ret, 0, 0)]);
        let frame = driver.frames.last().unwrap();
        let owner = driver.owner.clone();
        let ticket = session.allocate_action(
            &owner,
            &frame.ctx.scope,
            ActionKind::InternalInvocation,
            ResumePhase::ApplyOpcode,
        ).unwrap();
        driver.phase = DriverPhase::Awaiting {
            ticket: ticket.clone(),
            resume: ResumePhase::ApplyOpcode,
        };
        (session, driver, ticket)
    }

    #[test]
    fn obj_call_list_dispatch_mutates_and_honors_no_return() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::ObjCall, 2, 0),
            Bytecode::new(OpCode::ObjCall, 3, 1),
            Bytecode::new(OpCode::Ret, 0, 2),
        ]);
        let list_ref = session
            .with_player(1, |ctx| {
                ctx.player.alloc_datum(Datum::List(
                    crate::director::lingo::datum::DatumType::List,
                    std::collections::VecDeque::new(),
                    false,
                ))
            })
            .unwrap();
        let item_ref = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(17)))
            .unwrap();
        session
            .with_player(1, |ctx| {
                ctx.player.handler_stack_depth = 11;
                let scope = &mut ctx.player.scopes[0];
                scope.stack.push_value(StackDatum::Ref(list_ref.clone()));
                scope.stack.push_value(StackDatum::Ref(item_ref));
                scope.stack.push_value(StackDatum::ArgMarker { count: 2, no_ret: true });
            })
            .unwrap();

        assert!(matches!(driver.turn(&mut session), DriverTurn::Waiting));
        let after_append = session
            .with_player(1, |ctx| {
                let list = ctx.player.get_datum(&list_ref);
                (
                    matches!(list, Datum::List(_, items, _) if items.len() == 1),
                    ctx.player.scopes[0].stack.len(),
                    matches!(ctx.player.get_datum(&ctx.player.last_handler_result), Datum::Void),
                    ctx.player.handler_stack_depth,
                    ctx.player.scopes[0].bytecode_index,
                )
            })
            .unwrap();
        assert_eq!(after_append, (true, 0, true, 11, 1));

        session
            .with_player(1, |ctx| {
                let scope = &mut ctx.player.scopes[0];
                scope.stack.push_value(StackDatum::Ref(list_ref.clone()));
                scope.stack.push_value(StackDatum::ArgMarker { count: 1, no_ret: false });
            })
            .unwrap();
        assert!(matches!(driver.turn(&mut session), DriverTurn::Waiting));
        let count_result = session
            .with_player(1, |ctx| {
                let result = ctx.player.last_handler_result.clone();
                (
                    matches!(ctx.player.get_datum(&result), Datum::Int(1)),
                    ctx.player.handler_stack_depth,
                    ctx.player.scopes[0].bytecode_index,
                    ctx.player.scopes[0].stack.len(),
                )
            })
            .unwrap();
        assert_eq!(count_result, (true, 11, 2, 1));
        assert!(matches!(driver.turn(&mut session), DriverTurn::Complete(_)));
    }

    #[test]
    fn obj_call_void_dispatches_count_and_unknown_no_return_synchronously() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::ObjCall, 3, 0),
            Bytecode::new(OpCode::ObjCall, 4, 1),
            Bytecode::new(OpCode::Ret, 0, 2),
        ]);
        session
            .with_player(1, |ctx| {
                ctx.player.handler_stack_depth = 17;
                let scope = &mut ctx.player.scopes[0];
                scope.stack.push_value(StackDatum::Void);
                scope.stack.push_value(StackDatum::ArgMarker {
                    count: 1,
                    no_ret: false,
                });
            })
            .unwrap();

        assert!(matches!(driver.turn(&mut session), DriverTurn::Waiting));
        let count_result = session
            .with_player(1, |ctx| {
                let result = ctx.player.last_handler_result.clone();
                (
                    matches!(ctx.player.get_datum(&result), Datum::Int(0)),
                    ctx.player.handler_stack_depth,
                    ctx.player.scopes[0].bytecode_index,
                    ctx.player.scopes[0].stack.len(),
                )
            })
            .unwrap();
        assert_eq!(count_result, (true, 17, 1, 1));

        session
            .with_player(1, |ctx| {
                let scope = &mut ctx.player.scopes[0];
                scope.stack.push_value(StackDatum::Void);
                scope.stack.push_value(StackDatum::ArgMarker {
                    count: 1,
                    no_ret: true,
                });
            })
            .unwrap();
        assert!(matches!(driver.turn(&mut session), DriverTurn::Waiting));
        let unknown_result = session
            .with_player(1, |ctx| {
                let result = ctx.player.last_handler_result.clone();
                (
                    matches!(ctx.player.get_datum(&result), Datum::Void),
                    ctx.player.handler_stack_depth,
                    ctx.player.scopes[0].bytecode_index,
                    ctx.player.scopes[0].stack.len(),
                )
            })
            .unwrap();
        assert_eq!(unknown_result, (true, 17, 2, 1));
        assert!(matches!(driver.turn(&mut session), DriverTurn::Complete(_)));
    }

    #[test]
    fn obj_call_string_and_string_chunk_dispatch_synchronously() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::ObjCall, 3, 0),
            Bytecode::new(OpCode::ObjCall, 5, 1),
            Bytecode::new(OpCode::ObjCall, 10, 2),
            Bytecode::new(OpCode::Ret, 0, 3),
        ]);
        let (string_ref, operand_ref, chunk_ref, index_ref) = session
            .with_player(1, |ctx| {
                let string_ref = ctx.player.alloc_datum(Datum::String("one two".to_owned()));
                let operand_ref = ctx.player.alloc_datum(Datum::String("word".to_owned()));
                let chunk_ref = ctx.player.alloc_datum(Datum::StringChunk(
                    crate::director::lingo::datum::StringChunkSource::Datum(string_ref.clone()),
                    crate::director::lingo::datum::StringChunkExpr {
                        chunk_type: crate::director::lingo::datum::StringChunkType::Char,
                        start: 1,
                        end: 3,
                        item_delimiter: ',',
                    },
                    "one".to_owned(),
                ));
                let index_ref = ctx.player.alloc_datum(Datum::Int(2));
                (string_ref, operand_ref, chunk_ref, index_ref)
            })
            .unwrap();

        session
            .with_player(1, |ctx| {
                let scope = &mut ctx.player.scopes[0];
                scope.stack.push_value(StackDatum::Ref(string_ref));
                scope.stack.push_value(StackDatum::Ref(operand_ref));
                scope.stack.push_value(StackDatum::ArgMarker { count: 2, no_ret: false });
            })
            .unwrap();
        assert!(matches!(driver.turn(&mut session), DriverTurn::Waiting));
        let count = session
            .with_player(1, |ctx| ctx.player.get_datum(&ctx.player.last_handler_result).clone())
            .unwrap();
        assert!(matches!(count, Datum::Int(2)));

        session
            .with_player(1, |ctx| {
                let scope = &mut ctx.player.scopes[0];
                scope.stack.push_value(StackDatum::Ref(chunk_ref));
                scope.stack.push_value(StackDatum::Ref(index_ref));
                scope.stack.push_value(StackDatum::ArgMarker { count: 2, no_ret: false });
            })
            .unwrap();
        assert!(matches!(driver.turn(&mut session), DriverTurn::Waiting));
        let character = session
            .with_player(1, |ctx| ctx.player.get_datum(&ctx.player.last_handler_result).clone())
            .unwrap();
        assert!(matches!(character, Datum::String(value) if value == "n"));

        let mut foreign_session = RuntimeSession::new(SymbolOwner { session: 91, generation: 1 });
        let (foreign_tx, _foreign_rx) = channel::unbounded();
        assert!(foreign_session.add_player(9, foreign_tx));
        let foreign_ref = foreign_session
            .with_player(9, |ctx| ctx.player.alloc_datum(Datum::String("foreign".to_owned())))
            .unwrap();
        let local_start = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(1)))
            .unwrap();
        let local_end = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(1)))
            .unwrap();
        let chars_error = session
            .with_player(1, |mut ctx| {
                StringHandlers::chars(
                    &mut ctx,
                    &vec![foreign_ref.clone(), local_start.clone(), local_end.clone()],
                )
            })
            .unwrap()
            .unwrap_err();
        assert_eq!(chars_error.code, ScriptErrorCode::InvalidReference);

        let foreign_chunk = session
            .with_player(1, |ctx| {
                ctx.player.alloc_datum(Datum::StringChunk(
                    crate::director::lingo::datum::StringChunkSource::Datum(foreign_ref),
                    crate::director::lingo::datum::StringChunkExpr {
                        chunk_type: crate::director::lingo::datum::StringChunkType::Char,
                        start: 1,
                        end: 1,
                        item_delimiter: ',',
                    },
                    "f".to_owned(),
                ))
            })
            .unwrap();
        session
            .with_player(1, |ctx| {
                let scope = &mut ctx.player.scopes[0];
                scope.stack.push_value(StackDatum::Ref(foreign_chunk));
                scope.stack.push_value(StackDatum::ArgMarker { count: 1, no_ret: false });
            })
            .unwrap();
        assert!(matches!(driver.turn(&mut session), DriverTurn::Error(error) if error.code == ScriptErrorCode::InvalidReference));

        let mut foreign_symbols = crate::player::symbols::symbol_table::SymbolTable::new();
        let foreign_symbol = foreign_symbols.intern("foreignValue");
        let global_name = session.symbols_mut().intern("foreignGlobal");
        let value_error = match session
            .with_player(1, |mut ctx| {
                let foreign_value = ctx.player.alloc_datum(Datum::Symbol(foreign_symbol));
                let foreign_list = ctx.player.alloc_datum(Datum::List(
                    crate::director::lingo::datum::DatumType::List,
                    VecDeque::from([foreign_value]),
                    false,
                ));
                ctx.player.globals.insert(global_name.clone(), foreign_list);
                StringDatumUtils::get_built_in_prop(
                    &mut *ctx.player,
                    &mut *ctx.symbols,
                    "foreignGlobal * 2",
                    Symbol::builtin(BuiltInSymbol::Value),
                )
            })
            .unwrap()
        {
            Err(error) => error,
            Ok(_) => panic!("foreign nested value expression unexpectedly succeeded"),
        };
        assert_eq!(value_error.code, ScriptErrorCode::InvalidReference);

        let ordinary_value_fallback = session
            .with_player(1, |mut ctx| {
                StringDatumUtils::get_built_in_prop(
                    &mut *ctx.player,
                    &mut *ctx.symbols,
                    "[1 +]",
                    Symbol::builtin(BuiltInSymbol::Value),
                )
            })
            .unwrap()
            .unwrap();
        assert!(matches!(ordinary_value_fallback, Datum::String(value) if value == "[1 +]"));
    }

    #[test]
    fn obj_call_point_dispatches_count_synchronously() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::ObjCall, 3, 0),
            Bytecode::new(OpCode::Ret, 0, 1),
        ]);
        let point_ref = session
            .with_player(1, |ctx| {
                ctx.player
                    .alloc_datum(Datum::Point([12.5, -3.0], 1))
            })
            .unwrap();
        session
            .with_player(1, |ctx| {
                ctx.player.handler_stack_depth = 19;
                let scope = &mut ctx.player.scopes[0];
                scope.stack.push_value(StackDatum::Ref(point_ref));
                scope.stack.push_value(StackDatum::ArgMarker {
                    count: 1,
                    no_ret: false,
                });
            })
            .unwrap();

        assert!(matches!(driver.turn(&mut session), DriverTurn::Waiting));
        let count_result = session
            .with_player(1, |ctx| {
                let result = ctx.player.last_handler_result.clone();
                (
                    matches!(ctx.player.get_datum(&result), Datum::Int(2)),
                    ctx.player.handler_stack_depth,
                    ctx.player.scopes[0].bytecode_index,
                    ctx.player.scopes[0].stack.len(),
                )
            })
            .unwrap();
        assert_eq!(count_result, (true, 19, 1, 1));
        assert!(matches!(driver.turn(&mut session), DriverTurn::Complete(_)));
    }

    #[test]
    fn obj_call_point_set_get_preserves_flags_and_rejects_foreign_input() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::ObjCall, 6, 0),
            Bytecode::new(OpCode::ObjCall, 5, 1),
            Bytecode::new(OpCode::ObjCall, 6, 2),
        ]);
        let point_ref = session
            .with_player(1, |ctx| {
                ctx.player
                    .alloc_datum(Datum::Point([12.5, -3.0], 1))
            })
            .unwrap();
        let index_one = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(1)))
            .unwrap();
        let index_two = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(2)))
            .unwrap();
        let replacement = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Float(4.25)))
            .unwrap();
        session
            .with_player(1, |ctx| {
                ctx.player.handler_stack_depth = 29;
                let scope = &mut ctx.player.scopes[0];
                scope.stack.push_value(StackDatum::Ref(point_ref.clone()));
                scope.stack.push_value(StackDatum::Ref(index_two.clone()));
                scope.stack.push_value(StackDatum::Ref(replacement.clone()));
                scope.stack.push_value(StackDatum::ArgMarker {
                    count: 3,
                    no_ret: true,
                });
            })
            .unwrap();

        assert!(matches!(driver.turn(&mut session), DriverTurn::Waiting));
        let changed = session
            .with_player(1, |ctx| {
                let (values, flags) = ctx.player.get_datum(&point_ref).to_point_inline().unwrap();
                (values, flags, ctx.player.handler_stack_depth, ctx.player.scopes[0].stack.len())
            })
            .unwrap();
        assert_eq!(changed, ([12.5, 4.25], 3, 29, 0));

        session
            .with_player(1, |ctx| {
                let scope = &mut ctx.player.scopes[0];
                scope.stack.push_value(StackDatum::Ref(point_ref.clone()));
                scope.stack.push_value(StackDatum::Ref(index_one));
                scope.stack.push_value(StackDatum::ArgMarker {
                    count: 2,
                    no_ret: false,
                });
            })
            .unwrap();
        assert!(matches!(driver.turn(&mut session), DriverTurn::Waiting));
        let get_result = session
            .with_player(1, |ctx| {
                let result = ctx.player.last_handler_result.clone();
                (
                    matches!(ctx.player.get_datum(&result), Datum::Float(value) if (*value - 12.5).abs() < f64::EPSILON),
                    ctx.player.handler_stack_depth,
                    ctx.player.scopes[0].bytecode_index,
                    ctx.player.scopes[0].stack.len(),
                )
            })
            .unwrap();
        assert_eq!(get_result, (true, 29, 2, 1));

        let mut foreign_session = RuntimeSession::new(SymbolOwner { session: 8, generation: 1 });
        let (foreign_tx, _foreign_rx) = channel::unbounded();
        assert!(foreign_session.add_player(2, foreign_tx));
        let foreign_value = foreign_session
            .with_player(2, |ctx| ctx.player.alloc_datum(Datum::Int(9)))
            .unwrap();
        session
            .with_player(1, |ctx| {
                let scope = &mut ctx.player.scopes[0];
                scope.stack.push_value(StackDatum::Ref(point_ref.clone()));
                scope.stack.push_value(StackDatum::Ref(index_two));
                scope.stack.push_value(StackDatum::Ref(foreign_value));
                scope.stack.push_value(StackDatum::ArgMarker {
                    count: 3,
                    no_ret: false,
                });
            })
            .unwrap();
        assert!(matches!(driver.turn(&mut session), DriverTurn::Error(_)));
        let unchanged = session
            .with_player(1, |ctx| {
                let (values, flags) = ctx.player.get_datum(&point_ref).to_point_inline().unwrap();
                (values, flags, ctx.player.handler_stack_depth)
            })
            .unwrap();
        assert_eq!(unchanged, ([12.5, 4.25], 3, 29));
    }

    #[test]
    fn obj_call_point_error_does_not_mutate_receiver_or_depth() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::ObjCall, 4, 0),
            Bytecode::new(OpCode::Ret, 0, 1),
        ]);
        let point_ref = session
            .with_player(1, |ctx| {
                ctx.player
                    .alloc_datum(Datum::Point([12.5, -3.0], 1))
            })
            .unwrap();
        session
            .with_player(1, |ctx| {
                ctx.player.handler_stack_depth = 23;
                let scope = &mut ctx.player.scopes[0];
                scope.stack.push_value(StackDatum::Ref(point_ref.clone()));
                scope.stack.push_value(StackDatum::ArgMarker {
                    count: 1,
                    no_ret: false,
                });
            })
            .unwrap();

        assert!(matches!(driver.turn(&mut session), DriverTurn::Error(_)));
        let receiver_state = session
            .with_player(1, |ctx| {
                let (values, flags) = ctx.player.get_datum(&point_ref).to_point_inline().unwrap();
                (values, flags, ctx.player.handler_stack_depth)
            })
            .unwrap();
        assert_eq!(receiver_state, ([12.5, -3.0], 1, 23));
    }

    #[test]
    fn obj_call_vector_set_get_rejects_foreign_nested_input() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::ObjCall, 6, 0),
            Bytecode::new(OpCode::ObjCall, 5, 1),
            Bytecode::new(OpCode::ObjCall, 7, 2),
        ]);
        let vector_ref = session
            .with_player(1, |ctx| {
                ctx.player.alloc_datum(Datum::Vector([1.25, 2.0, 3.0]))
            })
            .unwrap();
        let index_two = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(2)))
            .unwrap();
        let replacement = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(9)))
            .unwrap();
        session
            .with_player(1, |ctx| {
                ctx.player.handler_stack_depth = 37;
                let scope = &mut ctx.player.scopes[0];
                scope.stack.push_value(StackDatum::Ref(vector_ref.clone()));
                scope.stack.push_value(StackDatum::Ref(index_two.clone()));
                scope.stack.push_value(StackDatum::Ref(replacement));
                scope.stack.push_value(StackDatum::ArgMarker {
                    count: 3,
                    no_ret: true,
                });
            })
            .unwrap();
        assert!(matches!(driver.turn(&mut session), DriverTurn::Waiting));

        session
            .with_player(1, |ctx| {
                let scope = &mut ctx.player.scopes[0];
                scope.stack.push_value(StackDatum::Ref(vector_ref.clone()));
                scope.stack.push_value(StackDatum::Ref(index_two));
                scope.stack.push_value(StackDatum::ArgMarker {
                    count: 2,
                    no_ret: false,
                });
            })
            .unwrap();
        assert!(matches!(driver.turn(&mut session), DriverTurn::Waiting));
        let readback = session
            .with_player(1, |ctx| {
                let result = ctx.player.last_handler_result.clone();
                (
                    matches!(ctx.player.get_datum(&result), Datum::Float(value) if (*value - 9.0).abs() < f64::EPSILON),
                    ctx.player.handler_stack_depth,
                )
            })
            .unwrap();
        assert_eq!(readback, (true, 37));

        let mut foreign_session = RuntimeSession::new(SymbolOwner { session: 10, generation: 1 });
        let (foreign_tx, _foreign_rx) = channel::unbounded();
        assert!(foreign_session.add_player(2, foreign_tx));
        let foreign_component = foreign_session
            .with_player(2, |ctx| ctx.player.alloc_datum(Datum::Int(4)))
            .unwrap();
        let mixed_list = session
            .with_player(1, |ctx| {
                let second = ctx.player.alloc_datum(Datum::Int(5));
                let third = ctx.player.alloc_datum(Datum::Int(6));
                ctx.player.alloc_datum(Datum::List(
                    crate::director::lingo::datum::DatumType::List,
                    std::collections::VecDeque::from([foreign_component, second, third]),
                    false,
                ))
            })
            .unwrap();
        session
            .with_player(1, |ctx| {
                let scope = &mut ctx.player.scopes[0];
                scope.stack.push_value(StackDatum::Ref(vector_ref.clone()));
                scope.stack.push_value(StackDatum::Ref(mixed_list));
                scope.stack.push_value(StackDatum::ArgMarker {
                    count: 2,
                    no_ret: false,
                });
            })
            .unwrap();
        assert!(matches!(driver.turn(&mut session), DriverTurn::Error(_)));
        let unchanged = session
            .with_player(1, |ctx| {
                (
                    matches!(ctx.player.get_datum(&vector_ref), Datum::Vector([x, y, z]) if (*x, *y, *z) == (1.25, 9.0, 3.0)),
                    ctx.player.handler_stack_depth,
                )
            })
            .unwrap();
        assert_eq!(unchanged, (true, 37));
    }

    #[test]
    fn obj_call_color_and_math_dispatch_synchronously() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::ObjCall, 8, 0),
            Bytecode::new(OpCode::ObjCall, 9, 1),
            Bytecode::new(OpCode::Ret, 0, 2),
        ]);
        let color_ref = session
            .with_player(1, |ctx| {
                ctx.player.alloc_datum(Datum::ColorRef(crate::player::sprite::ColorRef::Rgb(
                    0x12, 0x34, 0x56,
                )))
            })
            .unwrap();
        let math_ref = session
            .with_player(1, |ctx| {
                ctx.player
                    .math_objects
                    .insert(1, crate::player::handlers::datum_handlers::math::MathObject::new(1));
                ctx.player.alloc_datum(Datum::MathRef(1))
            })
            .unwrap();
        let half_pi = session
            .with_player(1, |ctx| {
                ctx.player.alloc_datum(Datum::Float(std::f64::consts::FRAC_PI_2))
            })
            .unwrap();
        session
            .with_player(1, |ctx| {
                ctx.player.handler_stack_depth = 41;
                let scope = &mut ctx.player.scopes[0];
                scope.stack.push_value(StackDatum::Ref(color_ref));
                scope.stack.push_value(StackDatum::ArgMarker {
                    count: 1,
                    no_ret: false,
                });
            })
            .unwrap();
        assert!(matches!(driver.turn(&mut session), DriverTurn::Waiting));
        let color_result = session
            .with_player(1, |ctx| {
                let result = ctx.player.last_handler_result.clone();
                (
                    matches!(ctx.player.get_datum(&result), Datum::String(value) if value == "#123456"),
                    ctx.player.handler_stack_depth,
                )
            })
            .unwrap();
        assert_eq!(color_result, (true, 41));

        session
            .with_player(1, |ctx| {
                let scope = &mut ctx.player.scopes[0];
                scope.stack.push_value(StackDatum::Ref(math_ref));
                scope.stack.push_value(StackDatum::Ref(half_pi));
                scope.stack.push_value(StackDatum::ArgMarker {
                    count: 2,
                    no_ret: false,
                });
            })
            .unwrap();
        assert!(matches!(driver.turn(&mut session), DriverTurn::Waiting));
        let math_result = session
            .with_player(1, |ctx| {
                let result = ctx.player.last_handler_result.clone();
                (
                    matches!(ctx.player.get_datum(&result), Datum::Float(value) if (*value - 1.0).abs() < f64::EPSILON),
                    ctx.player.handler_stack_depth,
                )
            })
            .unwrap();
        assert_eq!(math_result, (true, 41));
        assert!(matches!(driver.turn(&mut session), DriverTurn::Complete(_)));
    }

    #[test]
    fn obj_call_rect_set_get_preserves_flags_and_rejects_foreign_input() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::ObjCall, 6, 0),
            Bytecode::new(OpCode::ObjCall, 5, 1),
            Bytecode::new(OpCode::ObjCall, 6, 2),
        ]);
        let rect_ref = session
            .with_player(1, |ctx| {
                ctx.player
                    .alloc_datum(Datum::Rect([1.5, 2.0, 10.0, 20.0], 1))
            })
            .unwrap();
        let index_one = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(1)))
            .unwrap();
        let index_four = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(4)))
            .unwrap();
        let replacement = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Float(30.5)))
            .unwrap();
        session
            .with_player(1, |ctx| {
                ctx.player.handler_stack_depth = 31;
                let scope = &mut ctx.player.scopes[0];
                scope.stack.push_value(StackDatum::Ref(rect_ref.clone()));
                scope.stack.push_value(StackDatum::Ref(index_four.clone()));
                scope.stack.push_value(StackDatum::Ref(replacement.clone()));
                scope.stack.push_value(StackDatum::ArgMarker {
                    count: 3,
                    no_ret: true,
                });
            })
            .unwrap();

        assert!(matches!(driver.turn(&mut session), DriverTurn::Waiting));
        let changed = session
            .with_player(1, |ctx| {
                let (values, flags) = ctx.player.get_datum(&rect_ref).to_rect_inline().unwrap();
                (values, flags, ctx.player.handler_stack_depth, ctx.player.scopes[0].stack.len())
            })
            .unwrap();
        assert_eq!(changed, ([1.5, 2.0, 10.0, 30.5], 9, 31, 0));

        session
            .with_player(1, |ctx| {
                let scope = &mut ctx.player.scopes[0];
                scope.stack.push_value(StackDatum::Ref(rect_ref.clone()));
                scope.stack.push_value(StackDatum::Ref(index_one));
                scope.stack.push_value(StackDatum::ArgMarker {
                    count: 2,
                    no_ret: false,
                });
            })
            .unwrap();
        assert!(matches!(driver.turn(&mut session), DriverTurn::Waiting));
        let get_result = session
            .with_player(1, |ctx| {
                let result = ctx.player.last_handler_result.clone();
                (
                    matches!(ctx.player.get_datum(&result), Datum::Float(value) if (*value - 1.5).abs() < f64::EPSILON),
                    ctx.player.handler_stack_depth,
                    ctx.player.scopes[0].bytecode_index,
                    ctx.player.scopes[0].stack.len(),
                )
            })
            .unwrap();
        assert_eq!(get_result, (true, 31, 2, 1));

        let mut foreign_session = RuntimeSession::new(SymbolOwner { session: 9, generation: 1 });
        let (foreign_tx, _foreign_rx) = channel::unbounded();
        assert!(foreign_session.add_player(2, foreign_tx));
        let foreign_value = foreign_session
            .with_player(2, |ctx| ctx.player.alloc_datum(Datum::Int(99)))
            .unwrap();
        session
            .with_player(1, |ctx| {
                let scope = &mut ctx.player.scopes[0];
                scope.stack.push_value(StackDatum::Ref(rect_ref.clone()));
                scope.stack.push_value(StackDatum::Ref(index_four));
                scope.stack.push_value(StackDatum::Ref(foreign_value));
                scope.stack.push_value(StackDatum::ArgMarker {
                    count: 3,
                    no_ret: false,
                });
            })
            .unwrap();
        assert!(matches!(driver.turn(&mut session), DriverTurn::Error(_)));
        let unchanged = session
            .with_player(1, |ctx| {
                let (values, flags) = ctx.player.get_datum(&rect_ref).to_rect_inline().unwrap();
                (values, flags, ctx.player.handler_stack_depth)
            })
            .unwrap();
        assert_eq!(unchanged, ([1.5, 2.0, 10.0, 30.5], 9, 31));
    }

    #[test]
    fn obj_call_handled_error_balances_handler_depth() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::ObjCall, 4, 0),
            Bytecode::new(OpCode::Ret, 0, 1),
        ]);
        let list_ref = session
            .with_player(1, |ctx| {
                ctx.player.alloc_datum(Datum::List(
                    crate::director::lingo::datum::DatumType::List,
                    std::collections::VecDeque::new(),
                    false,
                ))
            })
            .unwrap();
        session
            .with_player(1, |ctx| {
                ctx.player.handler_stack_depth = 13;
                let scope = &mut ctx.player.scopes[0];
                scope.stack.push_value(StackDatum::Ref(list_ref));
                scope.stack.push_value(StackDatum::ArgMarker { count: 1, no_ret: false });
            })
            .unwrap();
        assert!(matches!(driver.turn(&mut session), DriverTurn::Error(_)));
        assert_eq!(session.with_player(1, |ctx| {
            ctx.player.handler_stack_depth
        }), Some(13));
    }

    #[test]
    fn obj_call_unsupported_keeps_real_payload_and_applies_owned_result_once() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::ObjCall, 4, 0),
            Bytecode::new(OpCode::Ret, 0, 1),
        ]);
        let receiver = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(9)))
            .unwrap();
        let argument = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(4)))
            .unwrap();
        session
            .with_player(1, |ctx| {
                let scope = &mut ctx.player.scopes[0];
                scope.stack.push_value(StackDatum::Ref(receiver.clone()));
                scope.stack.push_value(StackDatum::Ref(argument.clone()));
                scope.stack.push_value(StackDatum::ArgMarker { count: 2, no_ret: false });
            })
            .unwrap();

        let pending = match driver.turn(&mut session) {
            DriverTurn::Pending(PendingAction::Internal(request)) => request,
            _ => panic!("unsupported ObjCall did not suspend"),
        };
        let ticket = pending.ticket.clone();
        match pending.request {
            InternalVmRequest::Object { receiver: actual, name, args } => {
                assert_eq!(actual, receiver);
                assert_eq!(name, session.symbols_mut().intern("unsupportedCall"));
                assert_eq!(args, vec![argument]);
            }
            _ => panic!("unsupported ObjCall used the wrong request type"),
        }
        assert_eq!(session.with_player(1, |ctx| {
            (ctx.player.scopes[0].bytecode_index, ctx.player.scopes[0].stack.len())
        }), Some((0, 0)));

        let owned_result = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(23)))
            .unwrap();
        assert!(driver.complete(
            &mut session,
            ticket.clone(),
            ActionCompletion::InternalResult(owned_result),
        ));
        assert!(matches!(driver.turn(&mut session), DriverTurn::Waiting));
        let applied = session
            .with_player(1, |ctx| {
                (
                    matches!(ctx.player.get_datum(&ctx.player.last_handler_result), Datum::Int(23)),
                    ctx.player.scopes[0].bytecode_index,
                    ctx.player.scopes[0].stack.len(),
                )
            })
            .unwrap();
        assert_eq!(applied, (true, 1, 1));
        assert!(matches!(driver.turn(&mut session), DriverTurn::Complete(_)));
        assert!(!driver.complete(
            &mut session,
            ticket,
            ActionCompletion::InternalResult(DatumRef::Void),
        ));
    }

    #[test]
    fn obj_call_completion_rejects_same_owner_malformed_and_foreign_symbol_results() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::ObjCall, 4, 0),
            Bytecode::new(OpCode::Ret, 0, 1),
        ]);
        let receiver = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(9)))
            .unwrap();
        session
            .with_player(1, |ctx| {
                let scope = &mut ctx.player.scopes[0];
                scope.stack.push_value(StackDatum::Ref(receiver));
                scope.stack.push_value(StackDatum::ArgMarker { count: 1, no_ret: false });
            })
            .unwrap();
        let request = match driver.turn(&mut session) {
            DriverTurn::Pending(PendingAction::Internal(request)) => request,
            _ => panic!("unsupported ObjCall did not suspend"),
        };

        let (seed, stale) = session
            .with_player(1, |ctx| {
                let seed = ctx.player.alloc_datum(Datum::String("stale-result".to_owned()));
                let pointer = seed.ref_count_ptr().unwrap();
                let owner = seed.owner().unwrap().clone();
                // Keep the valid seed alive while the impossible id makes a
                // same-owner handle stale. `from_id` retains the seed's live
                // refcount, so dropping the rejected completion remains safe.
                let stale = DatumRef::from_id(usize::MAX, pointer, owner);
                (seed, stale)
            })
            .unwrap();
        assert!(!driver.complete(
            &mut session,
            request.ticket.clone(),
            ActionCompletion::InternalResult(stale),
        ));
        drop(seed);
        session.with_player(1, |ctx| ctx.player.drain_allocator_reclaims());
        assert!(session.action_details(&request.ticket).is_some());

        let mut foreign_symbols = crate::player::symbols::symbol_table::SymbolTable::new();
        let foreign_symbol = foreign_symbols.intern("foreignResultSymbol");
        let foreign_symbol_result = session
            .with_player(1, |ctx| {
                ctx.player.alloc_datum(Datum::Symbol(foreign_symbol))
            })
            .unwrap();
        assert!(!driver.complete(
            &mut session,
            request.ticket.clone(),
            ActionCompletion::InternalResult(foreign_symbol_result),
        ));
        assert!(matches!(driver.phase, DriverPhase::Awaiting { .. }));
        assert_eq!(session.with_player(1, |ctx| {
            (ctx.player.scopes[0].bytecode_index, ctx.player.scopes[0].stack.len())
        }), Some((0, 0)));
    }

    #[test]
    fn obj_call_completion_rejects_foreign_result_and_replacement_scope() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::ObjCall, 4, 0),
            Bytecode::new(OpCode::Ret, 0, 1),
        ]);
        let receiver = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(9)))
            .unwrap();
        session
            .with_player(1, |ctx| {
                let scope = &mut ctx.player.scopes[0];
                scope.stack.push_value(StackDatum::Ref(receiver));
                scope.stack.push_value(StackDatum::ArgMarker { count: 1, no_ret: false });
            })
            .unwrap();
        let ticket = match driver.turn(&mut session) {
            DriverTurn::Pending(PendingAction::Internal(request)) => request.ticket,
            _ => panic!("unsupported ObjCall did not suspend"),
        };
        assert!(session.add_player(2, channel::unbounded().0));
        let foreign = session
            .with_player(2, |ctx| ctx.player.alloc_datum(Datum::Int(31)))
            .unwrap();
        assert!(!driver.complete(
            &mut session,
            ticket.clone(),
            ActionCompletion::InternalResult(foreign),
        ));
        assert!(session.action_details(&ticket).is_some());

        let before = session
            .with_player(1, |ctx| {
                ctx.player.bump_scope_invalidation_epoch();
                ctx.player.pop_scope();
                ctx.player.push_scope();
                ctx.player.scopes[0].bytecode_index = 19;
                ctx.player.scopes[0].stack.push_value(StackDatum::Int(91));
                (ctx.player.scopes[0].bytecode_index, ctx.player.scopes[0].stack.len())
            })
            .unwrap();
        let replacement = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(32)))
            .unwrap();
        assert!(!driver.complete(
            &mut session,
            ticket,
            ActionCompletion::InternalResult(replacement),
        ));
        assert!(matches!(driver.turn(&mut session), DriverTurn::Error(_)));
        assert_eq!(session.with_player(1, |ctx| {
            (ctx.player.scopes[0].bytecode_index, ctx.player.scopes[0].stack.len())
        }), Some(before));
    }

    fn pending_call(driver: &DriverContinuation) -> super::super::PendingCall {
        pending_call_named(driver, driver.frames[0].handler_name.clone())
    }

    fn pending_call_named(
        driver: &DriverContinuation,
        handler_name: Symbol,
    ) -> super::super::PendingCall {
        super::super::PendingCall {
            receiver: None,
            handler_ref: (
                driver.frames[0].ctx.code.script.member_ref.clone(),
                handler_name,
            ),
            args: Vec::new(),
            use_raw_arg_list: true,
            push_return: true,
        }
    }

    #[test]
    fn local_call_runs_synchronously_and_driver_advances_parent_once() {
        let (mut session, driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::PushArgList, 0, 0),
            Bytecode::new(OpCode::LocalCall, 1, 1),
            Bytecode::new(OpCode::Ret, 0, 2),
        ]);
        session.insert_driver_for_test(driver);
        assert!(matches!(session.turn_handler(1), Some(DriverTurn::Waiting)));
        assert!(matches!(session.turn_handler(1), Some(DriverTurn::Waiting)));
        let (parent_pc, scope_count) = session
            .with_player(1, |ctx| {
                (ctx.player.scopes[0].bytecode_index, ctx.player.scope_count)
            })
            .unwrap();
        assert_eq!(parent_pc, 2);
        assert_eq!(scope_count, 2);
        assert!(matches!(session.turn_handler(1), Some(DriverTurn::Waiting)));
        assert!(matches!(session.turn_handler(1), Some(DriverTurn::Complete(_))));
    }

    #[test]
    fn empty_handler_pc_is_normal_completion() {
        let (mut session, mut driver) = prepared_running_driver(Vec::new());
        assert!(matches!(driver.turn(&mut session), DriverTurn::Complete(_)));
        assert_eq!(session.with_player(1, |ctx| ctx.player.scope_count), Some(0));
    }

    #[test]
    fn final_advance_finishes_without_incrementing_pc() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::PushZero, 0, 0),
            Bytecode::new(OpCode::PushZero, 0, 1),
        ]);
        assert!(matches!(driver.turn(&mut session), DriverTurn::Waiting));
        assert_eq!(session.with_player(1, |ctx| {
            (ctx.player.scopes[0].bytecode_index, ctx.player.scope_count)
        }), Some((1, 1)));
        assert!(matches!(driver.turn(&mut session), DriverTurn::Complete(_)));
        // Scope storage is retained after pop_scope, so the final PC is
        // inspectable even though the active depth has returned to zero.
        assert_eq!(session.with_player(1, |ctx| {
            (ctx.player.scopes[0].bytecode_index, ctx.player.scope_count)
        }), Some((1, 0)));
    }

    #[test]
    fn nonfinal_advance_increments_once_then_honors_stop_requested() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::PushZero, 0, 0),
            Bytecode::new(OpCode::PushZero, 0, 1),
        ]);
        session.with_player(1, |ctx| {
            ctx.player.scopes[0].stop_requested = true;
        }).unwrap();
        assert!(matches!(driver.turn(&mut session), DriverTurn::Complete(_)));
        assert_eq!(session.with_player(1, |ctx| {
            (ctx.player.scopes[0].bytecode_index, ctx.player.scope_count)
        }), Some((1, 0)));
    }

    #[test]
    fn child_success_advances_parent_once_and_delivers_return() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::PushZero, 0, 0),
        ]);
        let pending = pending_call(&driver);
        assert!(matches!(
            driver.start_child(&mut session, pending),
            DriverTurn::Waiting
        ));
        assert_eq!(driver.frames.len(), 2);
        assert_eq!(session.with_player(1, |ctx| {
            (ctx.player.scopes[0].bytecode_index, ctx.player.scope_count)
        }), Some((1, 2)));
        session.with_player(1, |ctx| {
            let value = ctx.player.alloc_datum(Datum::Int(7));
            ctx.player.scopes[1].return_value = value;
        }).unwrap();
        assert!(matches!(driver.finish_current_frame(&mut session), DriverTurn::Waiting));
        assert_eq!(driver.frames.len(), 1);
        let (parent_pc, returned_seven) = session.with_player(1, |ctx| {
            let value = ctx.player.scopes[0]
                .stack
                .pop_ref_with(&mut ctx.player.allocator, &mut ctx.player.bitmap_manager)
                .unwrap();
            (
                ctx.player.scopes[0].bytecode_index,
                matches!(ctx.player.get_datum(&value), Datum::Int(7)),
            )
        }).unwrap();
        assert_eq!(parent_pc, 1);
        assert!(returned_seven);
    }

    #[test]
    fn child_frame_failure_leaves_parent_pc_advanced_once_then_unwinds() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::PushZero, 0, 0),
        ]);
        let pending = pending_call(&driver);
        assert!(matches!(
            driver.start_child(&mut session, pending),
            DriverTurn::Waiting
        ));
        assert_eq!(session.with_player(1, |ctx| ctx.player.scopes[0].bytecode_index), Some(1));
        assert!(matches!(
            driver.fail_current_frame(&mut session, ScriptError::new("child failed".to_owned())),
            DriverTurn::Error(_)
        ));
        session.insert_driver_for_test(driver);
        assert!(matches!(session.turn_handler(1), Some(DriverTurn::Error(_))));
        assert_eq!(session.with_player(1, |ctx| {
            (ctx.player.scope_count, ctx.player.handler_stack_depth)
        }), Some((0, 0)));
    }

    #[test]
    fn early_virtual_child_advances_parent_without_pushing_frame() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::PushZero, 0, 0),
        ]);
        let member_ref = driver.frames[0].ctx.code.script.member_ref.clone();
        let call = session.symbols_mut().intern("call");
        session.with_player(1, |ctx| {
            super::super::virtual_scripts::VirtualScriptRegistry::attach(
                ctx.player,
                member_ref,
                Rc::new(super::super::virtual_scripts::javascript_proxy::JavascriptProxy),
            );
        }).unwrap();
        let pending = pending_call_named(&driver, call);
        assert!(matches!(
            driver.start_child(&mut session, pending),
            DriverTurn::Waiting
        ));
        assert_eq!(driver.frames.len(), 1);
        let (parent_pc, scope_count, returned_void) = session.with_player(1, |ctx| {
            let value = ctx.player.scopes[0]
                .stack
                .pop_ref_with(&mut ctx.player.allocator, &mut ctx.player.bitmap_manager)
                .unwrap();
            (
                ctx.player.scopes[0].bytecode_index,
                ctx.player.scope_count,
                matches!(ctx.player.get_datum(&value), Datum::Void),
            )
        }).unwrap();
        assert_eq!(parent_pc, 1);
        assert_eq!(scope_count, 1);
        assert!(returned_void);
    }

    #[test]
    fn missing_child_handler_errors_after_parent_pc_advanced_once() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::PushZero, 0, 0),
        ]);
        let missing = session.symbols_mut().intern("missingChildHandler");
        let pending = pending_call_named(&driver, missing);
        assert!(matches!(
            driver.start_child(&mut session, pending),
            DriverTurn::Error(_)
        ));
        assert_eq!(driver.frames.len(), 1);
        assert_eq!(session.with_player(1, |ctx| {
            (ctx.player.scopes[0].bytecode_index, ctx.player.scope_count)
        }), Some((1, 1)));
        session.insert_driver_for_test(driver);
        assert!(matches!(session.turn_handler(1), Some(DriverTurn::Error(_))));
        assert_eq!(session.with_player(1, |ctx| {
            (ctx.player.scope_count, ctx.player.handler_stack_depth)
        }), Some((0, 0)));
    }

    #[test]
    fn stale_parent_rejects_child_without_mutating_replacement() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::PushZero, 0, 0),
        ]);
        let pending = pending_call(&driver);
        let before = session.with_player(1, |ctx| {
            ctx.player.pop_scope();
            ctx.player.push_scope();
            ctx.player.scopes[0].bytecode_index = 19;
            let sentinel = ctx.player.alloc_datum(Datum::Int(91));
            ctx.player.scopes[0].stack.push_value(
                super::super::scope::StackDatum::Ref(sentinel),
            );
            ctx.player.handler_stack_depth = 23;
            ctx.player.in_frame_script = true;
            let stack_ref = ctx.player.scopes[0]
                .stack
                .last_ref_with(&mut ctx.player.allocator, &mut ctx.player.bitmap_manager);
            (
                ctx.player.scope_count,
                ctx.player.scopes[0].bytecode_index,
                ctx.player.scopes[0].stack.len(),
                stack_ref
                    .as_ref()
                    .is_some_and(|r| matches!(ctx.player.get_datum(r), Datum::Int(91))),
                ctx.player.handler_stack_depth,
                ctx.player.in_frame_script,
            )
        });
        assert!(matches!(
            driver.start_child(&mut session, pending),
            DriverTurn::Error(_)
        ));
        assert_eq!(driver.frames.len(), 1);
        assert_eq!(session.with_player(1, |ctx| {
            (
                ctx.player.scope_count,
                ctx.player.scopes[0].bytecode_index,
                ctx.player.scopes[0].stack.len(),
                ctx.player.scopes[0]
                    .stack
                    .last_ref_with(&mut ctx.player.allocator, &mut ctx.player.bitmap_manager)
                    .as_ref()
                    .is_some_and(|r| matches!(ctx.player.get_datum(r), Datum::Int(91))),
                ctx.player.handler_stack_depth,
                ctx.player.in_frame_script,
            )
        }), before);
    }

    #[test]
    fn wrong_completion_preserves_pending_ticket_then_owned_result_resumes() {
        let (mut session, mut driver, ticket) = prepared_driver();
        assert!(!driver.complete(&mut session, ticket.clone(), ActionCompletion::Resume));
        assert!(session.action_details(&ticket).is_some());
        assert!(matches!(driver.phase, DriverPhase::Awaiting { .. }));

        session.add_player(2, channel::unbounded().0);
        let foreign = session.with_player(2, |ctx| ctx.player.alloc_datum(Datum::Int(9))).unwrap();
        assert!(!driver.complete(
            &mut session,
            ticket.clone(),
            ActionCompletion::InternalResult(foreign),
        ));
        assert!(session.action_details(&ticket).is_some());
        let owned = session.with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(3))).unwrap();
        assert!(driver.complete(
            &mut session,
            ticket.clone(),
            ActionCompletion::InternalResult(owned),
        ));
        match &driver.phase {
            DriverPhase::Resuming {
                resume: ResumePhase::ApplyOpcode,
                completion: ActionCompletion::InternalResult(result),
            } => {
                assert!(session.with_player(1, |ctx| {
                    matches!(ctx.player.get_datum(result), Datum::Int(3))
                }).unwrap());
            }
            _ => panic!("accepted completion was not retained for resumption"),
        }
        assert!(session.action_details(&ticket).is_none());
        assert!(!driver.complete(&mut session, ticket, ActionCompletion::InternalResult(DatumRef::Void)));
    }

    #[test]
    fn session_completion_rejection_keeps_driver_and_ticket() {
        let (mut session, driver, ticket) = prepared_driver();
        session.insert_driver_for_test(driver);
        assert!(!session.complete_handler_action(ticket.clone(), ActionCompletion::Resume));
        assert!(session.action_details(&ticket).is_some());
        assert!(matches!(session.turn_handler(1), Some(DriverTurn::Waiting)));
        assert!(session.action_details(&ticket).is_some());
        assert!(session.complete_handler_action(
            ticket.clone(),
            ActionCompletion::InternalResult(DatumRef::Void),
        ));
        assert!(matches!(session.turn_handler(1), Some(DriverTurn::Waiting)));
        assert!(session.action_details(&ticket).is_none());
    }

    #[test]
    fn terminal_error_unwinds_current_and_parent_frames_and_actions() {
        let (mut session, mut driver, ticket) = prepared_driver();
        let parent_scope = driver.frames[0].ctx.scope.clone();
        let expectation = session.with_player(1, |ctx| {
            super::super::SetupExpectation::capture(ctx.player, Some(&parent_scope))
        }).unwrap();
        let child = match super::super::setup_handler_frame(
            &mut session,
            1,
            None,
            (
                driver.frames[0].ctx.code.script.member_ref.clone(),
                driver.frames[0].handler_name.clone(),
            ),
            &Vec::new(),
            true,
            false,
            expectation,
        ).unwrap() {
            super::super::FrameSetup::Frame(frame) => frame,
            super::super::FrameSetup::Early(_) => panic!("test handler unexpectedly answered early"),
            super::super::FrameSetup::Pending(_) => panic!("test handler unexpectedly suspended"),
        };
        driver.frames.push(child);
        driver.phase = DriverPhase::Failed(ScriptError::new("test failure".to_owned()));
        session.insert_driver_for_test(driver);
        assert!(matches!(session.turn_handler(1), Some(DriverTurn::Error(_))));
        assert!(session.action_details(&ticket).is_none());
        assert!(session.with_player(1, |ctx| {
            ctx.player.scope_count == 0
                && ctx.player.handler_stack_depth == 0
                && !ctx.player.in_frame_script
        }).unwrap());
    }

    #[test]
    fn stale_child_finish_does_not_unwind_valid_parent() {
        let (mut session, mut driver, _ticket) = prepared_driver();
        let parent_scope = driver.frames[0].ctx.scope.clone();
        let expectation = session.with_player(1, |ctx| {
            super::super::SetupExpectation::capture(ctx.player, Some(&parent_scope))
        }).unwrap();
        let child = match super::super::setup_handler_frame(
            &mut session,
            1,
            None,
            (
                driver.frames[0].ctx.code.script.member_ref.clone(),
                driver.frames[0].handler_name.clone(),
            ),
            &Vec::new(),
            true,
            false,
            expectation,
        ).unwrap() {
            super::super::FrameSetup::Frame(frame) => frame,
            super::super::FrameSetup::Early(_) => panic!("test handler unexpectedly answered early"),
            super::super::FrameSetup::Pending(_) => panic!("test handler unexpectedly suspended"),
        };
        driver.frames.push(child);
        session.with_player(1, |ctx| {
            assert_eq!(ctx.player.scope_count, 2);
            ctx.player.pop_scope();
        }).unwrap();
        let before = session.with_player(1, |ctx| {
            (
                ctx.player.scope_count,
                ctx.player.handler_stack_depth,
                ctx.player.in_frame_script,
            )
        }).unwrap();
        assert!(matches!(driver.finish_current_frame(&mut session), DriverTurn::Error(_)));
        assert_eq!(driver.frames.len(), 2);
        assert!(!driver.cancel(&mut session));
        assert_eq!(session.with_player(1, |ctx| {
            (
                ctx.player.scope_count,
                ctx.player.handler_stack_depth,
                ctx.player.in_frame_script,
            )
        }).unwrap(), before);
    }

    #[test]
    fn foreign_capability_and_stale_scope_do_not_consume_action() {
        let (mut session, mut driver, ticket) = prepared_driver();
        let forged = CompletionTicket {
            id: ticket.id,
            capability: Arc::new(ActionCapability),
        };
        assert!(!driver.complete(
            &mut session,
            forged,
            ActionCompletion::InternalResult(DatumRef::Void),
        ));
        assert!(session.action_details(&ticket).is_some());

        let (_, _, foreign_ticket) = prepared_driver();
        assert!(!driver.complete(
            &mut session,
            foreign_ticket,
            ActionCompletion::InternalResult(DatumRef::Void),
        ));
        assert!(session.action_details(&ticket).is_some());

        let old_generation = driver.frames[0].ctx.scope.generation();
        session.with_player(1, |ctx| {
            ctx.player.pop_scope();
            ctx.player.push_scope();
            assert_ne!(ctx.player.scopes[0].generation, old_generation);
        }).unwrap();
        assert!(!driver.complete(
            &mut session,
            ticket.clone(),
            ActionCompletion::InternalResult(DatumRef::Void),
        ));
        assert!(session.action_details(&ticket).is_some());
    }

    #[test]
    fn epoch_invalidation_rejects_same_slot_and_generation() {
        let (mut session, mut driver, ticket) = prepared_driver();
        let old_generation = driver.frames[0].ctx.scope.generation();
        session.with_player(1, |ctx| {
            assert_eq!(ctx.player.scope_count, 1);
            assert_eq!(ctx.player.scopes[0].generation, old_generation);
            ctx.player.bump_scope_invalidation_epoch();
            assert_eq!(ctx.player.scope_count, 1);
            assert_eq!(ctx.player.scopes[0].generation, old_generation);
        }).unwrap();
        assert!(!driver.complete(
            &mut session,
            ticket.clone(),
            ActionCompletion::InternalResult(DatumRef::Void),
        ));
        assert!(session.action_details(&ticket).is_some());
    }

    #[test]
    fn non_top_scope_does_not_consume_action() {
        let (mut session, mut driver, ticket) = prepared_driver();
        session.with_player(1, |ctx| { ctx.player.push_scope(); }).unwrap();
        assert!(!driver.complete(
            &mut session,
            ticket.clone(),
            ActionCompletion::InternalResult(DatumRef::Void),
        ));
        assert!(session.action_details(&ticket).is_some());
    }

    #[test]
    fn removing_and_readding_player_retires_old_owner_action() {
        let (mut session, mut driver, ticket) = prepared_driver();
        driver.frames[0].is_frame_script = true;
        session.with_player(1, |ctx| {
            ctx.player.in_frame_script = true;
        }).unwrap();
        session.insert_driver_for_test(driver);
        assert!(session.action_details(&ticket).is_some());
        let removed = session.remove_player(1).expect("owned player should be removed");
        assert_eq!(removed.scope_count, 0);
        assert_eq!(removed.handler_stack_depth, 0);
        assert!(!removed.in_frame_script);
        assert!(session.action_details(&ticket).is_none());
        assert!(session.add_player(1, channel::unbounded().0));
        assert!(session.action_details(&ticket).is_none());
    }

    #[test]
    fn stale_driver_removal_preserves_current_scope_bookkeeping() {
        let (mut session, driver, ticket) = prepared_driver();
        session.insert_driver_for_test(driver);
        let before = session.with_player(1, |ctx| {
            ctx.player.handler_stack_depth = 11;
            ctx.player.in_frame_script = true;
            ctx.player.scopes[0].stack.push_void();
            ctx.player.scope_count
        }).unwrap();
        session.with_player(1, |ctx| ctx.player.bump_scope_invalidation_epoch()).unwrap();
        let returned = session.remove_player(1).expect("owned player should be removed");
        assert_eq!(returned.scope_count, before);
        assert_eq!(returned.handler_stack_depth, 11);
        assert!(returned.in_frame_script);
        assert_eq!(returned.scopes[0].stack.len(), 1);
        assert!(session.action_details(&ticket).is_none());
        assert!(session.add_player(1, channel::unbounded().0));
        assert!(session.with_player(1, |ctx| {
            ctx.player.scope_count == 0
                && ctx.player.handler_stack_depth == 0
                && !ctx.player.in_frame_script
        }).unwrap());
    }

    #[test]
    fn invalidated_pending_turn_retires_without_touching_replacement() {
        let (mut session, driver, ticket) = prepared_driver();
        session.insert_driver_for_test(driver);
        let before = session.with_player(1, |ctx| {
            // Invalidate the old token, then reuse its exact scope slot. The
            // player and allocator owners remain coherent while the current
            // replacement scope carries observable sentinel state.
            ctx.player.bump_scope_invalidation_epoch();
            ctx.player.pop_scope();
            ctx.player.push_scope();
            ctx.player.handler_stack_depth = 17;
            ctx.player.in_frame_script = true;
            ctx.player.scopes[0].stack.push_void();
            (
                ctx.player.scope_count,
                ctx.player.handler_stack_depth,
                ctx.player.in_frame_script,
                ctx.player.scopes[0].stack.len(),
            )
        }).unwrap();
        assert!(!session.complete_handler_action(
            ticket.clone(),
            ActionCompletion::InternalResult(DatumRef::Void),
        ));
        assert!(session.action_details(&ticket).is_some());
        assert!(matches!(session.turn_handler(1), Some(DriverTurn::Error(_))));
        assert!(session.action_details(&ticket).is_none());
        assert_eq!(session.with_player(1, |ctx| {
            (
                ctx.player.scope_count,
                ctx.player.handler_stack_depth,
                ctx.player.in_frame_script,
                ctx.player.scopes[0].stack.len(),
            )
        }).unwrap(), before);
        assert!(session.turn_handler(1).is_none());
    }

    #[test]
    fn internal_ext_call_preserves_name_args_return_and_duplicate_guard() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::ExtCall, 13, 0),
            Bytecode::new(OpCode::Ret, 0, 1),
        ]);
        let argument = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(17)))
            .unwrap();
        session
            .with_player(1, |ctx| {
                let scope = &mut ctx.player.scopes[0];
                scope.stack.push_value(StackDatum::Ref(argument.clone()));
                scope.stack.push_value(StackDatum::ArgMarker {
                    count: 1,
                    no_ret: false,
                });
            })
            .unwrap();
        let pending = match driver.turn(&mut session) {
            DriverTurn::Pending(PendingAction::Internal(request)) => request,
            _ => panic!("ExtCall did not produce an internal request"),
        };
        assert_eq!(session.with_player(1, |ctx| ctx.player.handler_stack_depth), Some(2));
        let ticket = pending.ticket.clone();
        match pending.request {
            InternalVmRequest::MovieAsync(request) => {
                assert_eq!(request.player_id, 1);
                assert!(request.owner.same_identity(
                    &session
                        .with_player(1, |ctx| ctx.player.owner.clone())
                        .expect("ExtCall owner disappeared")
                ));
                assert_eq!(request.kind, super::super::handlers::movie::MovieAsyncKind::Nothing);
                assert_eq!(request.args, vec![argument]);
            }
            _ => panic!("ExtCall used the wrong typed internal request"),
        }
        let result = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(23)))
            .unwrap();
        assert!(driver.complete(
            &mut session,
            ticket.clone(),
            ActionCompletion::InternalResult(result.clone()),
        ));
        assert!(matches!(driver.turn(&mut session), DriverTurn::Waiting));
        session.with_player(1, |ctx| {
            assert_eq!(ctx.player.handler_stack_depth, 1);
            assert!(matches!(ctx.player.get_datum(&ctx.player.last_handler_result), Datum::Int(23)));
            assert!(matches!(ctx.player.get_datum(&ctx.player.scopes[0].return_value), Datum::Int(23)));
            assert_eq!(ctx.player.scopes[0].stack.len(), 1);
        });
        assert!(!driver.complete(
            &mut session,
            ticket,
            ActionCompletion::InternalResult(result),
        ));
    }

    #[test]
    fn internal_ext_call_sync_voidp_no_return_updates_scope_without_stack_result() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::ExtCall, 14, 0),
            Bytecode::new(OpCode::Ret, 0, 1),
        ]);
        session.with_player(1, |ctx| {
            ctx.player.scopes[0].stack.push_value(StackDatum::Ref(DatumRef::Void));
            ctx.player.scopes[0].stack.push_value(StackDatum::ArgMarker {
                count: 1,
                no_ret: true,
            });
        });
        assert!(matches!(driver.turn(&mut session), DriverTurn::Waiting));
        session.with_player(1, |ctx| {
            assert_eq!(ctx.player.handler_stack_depth, 1);
            assert!(matches!(ctx.player.get_datum(&ctx.player.last_handler_result), Datum::Int(1)));
            assert!(matches!(ctx.player.get_datum(&ctx.player.scopes[0].return_value), Datum::Int(1)));
            assert_eq!(ctx.player.scopes[0].stack.len(), 0);
        });
    }

    #[test]
    fn internal_tell_local_sync_result_updates_scope_last_and_depth() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::TellCall, 14, 0),
            Bytecode::new(OpCode::Ret, 0, 1),
        ]);
        session.with_player(1, |ctx| {
            ctx.player.scopes[0].stack.push_value(StackDatum::Ref(DatumRef::Void));
            ctx.player.scopes[0].stack.push_value(StackDatum::ArgMarker {
                count: 1,
                no_ret: false,
            });
        });
        assert!(matches!(driver.turn(&mut session), DriverTurn::Waiting));
        session.with_player(1, |ctx| {
            assert_eq!(ctx.player.handler_stack_depth, 1);
            assert!(matches!(ctx.player.get_datum(&ctx.player.last_handler_result), Datum::Int(1)));
            assert!(matches!(ctx.player.get_datum(&ctx.player.scopes[0].return_value), Datum::Int(1)));
            assert_eq!(ctx.player.scopes[0].stack.len(), 1);
        });
    }

    #[test]
    fn internal_global_script_child_uses_raw_args_and_tears_down() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::ExtCall, 1, 0),
            Bytecode::new(OpCode::Ret, 0, 1),
        ]);
        let argument = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(27)))
            .unwrap();
        session.with_player(1, |ctx| {
            ctx.player.scopes[0].stack.push_value(StackDatum::Ref(argument.clone()));
            ctx.player.scopes[0].stack.push_value(StackDatum::ArgMarker {
                count: 1,
                no_ret: false,
            });
        });
        assert!(matches!(driver.turn(&mut session), DriverTurn::Waiting));
        assert_eq!(driver.frames.len(), 2);
        assert_eq!(session.with_player(1, |ctx| ctx.player.handler_stack_depth), Some(3));
        assert_eq!(session.with_player(1, |ctx| ctx.player.scopes[1].args.clone()), Some(vec![argument]));
        assert!(matches!(driver.turn(&mut session), DriverTurn::Waiting));
        assert_eq!(driver.frames.len(), 1);
        assert_eq!(session.with_player(1, |ctx| ctx.player.handler_stack_depth), Some(1));
        assert!(matches!(driver.turn(&mut session), DriverTurn::Complete(_)));
        assert_eq!(session.with_player(1, |ctx| ctx.player.handler_stack_depth), Some(0));

        let (mut early_session, mut early_driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::ExtCall, 1, 0),
            Bytecode::new(OpCode::Ret, 0, 1),
        ]);
        early_session.with_player(1, |ctx| {
            VirtualScriptRegistry::attach(
                ctx.player,
                CastMemberRef { cast_lib: 1, cast_member: 1 },
                Rc::new(EarlyChild),
            );
            ctx.player.scopes[0].stack.push_value(StackDatum::ArgMarker {
                count: 0,
                no_ret: false,
            });
        }).unwrap();
        assert!(matches!(early_driver.turn(&mut early_session), DriverTurn::Waiting));
        assert_eq!(early_driver.frames.len(), 1);
        early_session.with_player(1, |ctx| {
            assert_eq!(ctx.player.handler_stack_depth, 1);
            assert!(matches!(ctx.player.get_datum(&ctx.player.last_handler_result), Datum::Int(31)));
            assert!(matches!(ctx.player.get_datum(&ctx.player.scopes[0].return_value), Datum::Int(31)));
            assert_eq!(ctx.player.scopes[0].stack.len(), 1);
        });
    }

    #[test]
    fn internal_global_virtual_new_precedes_async_builtin_and_reset_decline_aborts() {
        let (mut session, _driver) = prepared_running_driver(vec![Bytecode::new(OpCode::Ret, 0, 0)]);
        session.with_player(1, |ctx| {
            VirtualScriptRegistry::register(ctx.player, "virtual-new", Rc::new(VirtualNew));
        }).unwrap();
        let result = session.dispatch_global(1, &Symbol::builtin(BuiltInSymbol::New), &[]);
        let result = match result {
            Ok(GlobalDispatch::SyncResult(Ok(result))) => result,
            _ => panic!("virtual New did not win precedence"),
        };
        assert_eq!(
            session.with_player(1, |ctx| matches!(ctx.player.get_datum(&result), Datum::Int(17))),
            Some(true)
        );

        let (mut reset_session, _reset_driver) =
            prepared_running_driver(vec![Bytecode::new(OpCode::Ret, 0, 0)]);
        reset_session.with_player(1, |ctx| {
            VirtualScriptRegistry::register(ctx.player, "reset-decline", Rc::new(ResetAndDecline));
        }).unwrap();
        let result = reset_session.dispatch_global(1, &Symbol::builtin(BuiltInSymbol::New), &[]);
        assert!(matches!(result, Err(ScriptError { code: ScriptErrorCode::Abort, .. })));
    }

    #[test]
    fn internal_global_rejects_foreign_cached_stage_instance_list() {
        let (mut session, _driver) =
            prepared_running_driver(vec![Bytecode::new(OpCode::Ret, 0, 0)]);
        let active_name = session.symbols_mut().intern("activeOnly");
        let active_handler = Rc::new(HandlerDef {
            name_id: 2,
            bytecode_array: vec![Bytecode::new(OpCode::Ret, 0, 0)],
            bytecode_index_map: fxhash::FxHashMap::default(),
            argument_name_ids: vec![],
            local_name_ids: vec![],
            global_name_ids: vec![],
            compiled_ir: RefCell::new(None),
        });
        let active_script_ref = CastMemberRef { cast_lib: 1, cast_member: 2 };
        let mut active_handlers = fxhash::FxHashMap::default();
        active_handlers.insert(active_name.clone(), active_handler);
        let active_script = Rc::new(Script {
            member_ref: active_script_ref,
            name: "active-only".to_owned(),
            chunk: ScriptChunk {
                script_number: 2,
                literals: vec![],
                handlers: vec![],
                property_name_ids: vec![],
                property_defaults: HashMap::new(),
            },
            script_type: ScriptType::Parent,
            handlers: active_handlers,
            handler_names_raw: vec!["activeOnly".to_owned()],
            handler_names: vec![active_name.clone()],
            properties: RefCell::new(fxhash::FxHashMap::default()),
        });
        let mut foreign = RuntimeSession::new(SymbolOwner { session: 98, generation: 1 });
        assert!(foreign.add_player(1, channel::unbounded().0));
        let foreign_list = foreign
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::List(
                crate::director::lingo::datum::DatumType::List,
                VecDeque::new(),
                false,
            )))
            .unwrap();
        session.with_player(1, |ctx| {
            ctx.player.movie.cast_manager.casts[0].scripts.insert(2, active_script);
            ctx.player.movie.cast_manager.clear_movie_script_cache();
            ctx.player.movie.score.channels.push(SpriteChannel::new(0));
            let mut active_channel = SpriteChannel::new(1);
            active_channel.sprite.entered = true;
            ctx.player.movie.score.channels.push(active_channel);
            ctx.player.script_instance_list_cache.insert(1, foreign_list);
        }).unwrap();
        let result = session.dispatch_global(1, &active_name, &[]);
        assert!(matches!(
            result,
            Err(ScriptError {
                code: ScriptErrorCode::InvalidReference,
                ..
            })
        ));
    }

    #[test]
    fn internal_ext_call_cancel_balances_legacy_layer() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::ExtCall, 13, 0),
            Bytecode::new(OpCode::Ret, 0, 1),
        ]);
        session.with_player(1, |ctx| {
            ctx.player.handler_stack_depth = 9;
            ctx.player.scopes[0].stack.push_value(StackDatum::ArgMarker {
                count: 0,
                no_ret: true,
            });
        });
        assert!(matches!(driver.turn(&mut session), DriverTurn::Pending(_)));
        assert_eq!(session.with_player(1, |ctx| ctx.player.handler_stack_depth), Some(10));
        assert!(driver.cancel(&mut session));
        assert_eq!(session.with_player(1, |ctx| ctx.player.handler_stack_depth), Some(8));
    }

    #[test]
    fn internal_ext_call_layer_does_not_touch_replacement_or_reset_depth() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::ExtCall, 13, 0),
            Bytecode::new(OpCode::Ret, 0, 1),
        ]);
        session.with_player(1, |ctx| {
            ctx.player.scopes[0].stack.push_value(StackDatum::ArgMarker {
                count: 0,
                no_ret: true,
            });
        });
        assert!(matches!(driver.turn(&mut session), DriverTurn::Pending(_)));
        session.with_player(1, |ctx| {
            ctx.player.bump_scope_invalidation_epoch();
            ctx.player.pop_scope();
            ctx.player.push_scope();
            ctx.player.handler_stack_depth = 17;
        }).unwrap();
        assert!(matches!(driver.turn(&mut session), DriverTurn::Error(_)));
        assert_eq!(session.with_player(1, |ctx| ctx.player.handler_stack_depth), Some(17));

        let (mut reset_session, mut reset_driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::ExtCall, 13, 0),
            Bytecode::new(OpCode::Ret, 0, 1),
        ]);
        reset_session.with_player(1, |ctx| {
            ctx.player.scopes[0].stack.push_value(StackDatum::ArgMarker {
                count: 0,
                no_ret: true,
            });
        });
        assert!(matches!(reset_driver.turn(&mut reset_session), DriverTurn::Pending(_)));
        reset_session.with_player(1, |ctx| {
            ctx.player.owner.begin_reset();
            ctx.player.handler_stack_depth = 23;
        }).unwrap();
        assert!(matches!(reset_driver.turn(&mut reset_session), DriverTurn::Error(_)));
        assert_eq!(reset_session.with_player(1, |ctx| ctx.player.handler_stack_depth), Some(23));
    }

    #[test]
    fn internal_new_obj_normalizes_script_and_ignores_no_return() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::NewObj, 11, 0),
            Bytecode::new(OpCode::Ret, 0, 1),
        ]);
        let script_arg = session
            .with_player(1, |ctx| {
                ctx.player.alloc_datum(Datum::CastMember(CastMemberRef {
                    cast_lib: 1,
                    cast_member: 1,
                }))
            })
            .unwrap();
        session
            .with_player(1, |ctx| {
                let scope = &mut ctx.player.scopes[0];
                scope.stack.push_value(StackDatum::Ref(script_arg));
                scope.stack.push_value(StackDatum::ArgMarker {
                    count: 1,
                    no_ret: true,
                });
                scope.return_value = DatumRef::Void;
            })
            .unwrap();
        let pending = match driver.turn(&mut session) {
            DriverTurn::Pending(PendingAction::Internal(request)) => request,
            _ => panic!("NewObj did not produce an internal request"),
        };
        match pending.request {
            InternalVmRequest::Construct { script, args } => {
                let is_script_ref = session
                    .with_player(1, |ctx| matches!(ctx.player.get_datum(&script), Datum::ScriptRef(_)))
                    .unwrap();
                assert!(is_script_ref);
                assert!(args.is_empty());
            }
            _ => panic!("NewObj used the wrong internal request"),
        }
        let result = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(77)))
            .unwrap();
        assert!(driver.complete(
            &mut session,
            pending.ticket.clone(),
            ActionCompletion::InternalResult(result),
        ));
        assert!(matches!(driver.turn(&mut session), DriverTurn::Waiting));
        session.with_player(1, |ctx| {
            assert_eq!(ctx.player.scopes[0].stack.len(), 1);
            assert!(matches!(ctx.player.get_datum(&ctx.player.last_handler_result), Datum::Void));
        });
    }

    #[test]
    fn internal_set_property_completion_ignores_result_and_last_handler() {
        // SetProperty is a write-only internal effect.  Keep this policy test
        // on a prepared internal ticket instead of pretending that the
        // CastLoad action emitted by a FileName setter is an Internal action.
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::Ret, 0, 0),
            Bytecode::new(OpCode::PushInt8, 1, 1),
        ]);
        let frame = driver.frames.last().unwrap();
        let ticket = session
            .allocate_action(
                &driver.owner,
                &frame.ctx.scope,
                ActionKind::InternalInvocation,
                ResumePhase::ApplyOpcode,
            )
            .unwrap();
        driver.phase = DriverPhase::Awaiting {
            ticket: ticket.clone(),
            resume: ResumePhase::ApplyOpcode,
        };
        let old_last = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(9)))
            .unwrap();
        session
            .with_player(1, |ctx| {
                ctx.player.last_handler_result = old_last.clone();
            })
            .unwrap();
        driver.internal_effect = Some(InternalResultEffect::SetProperty);
        let mut foreign = RuntimeSession::new(SymbolOwner { session: 91, generation: 1 });
        assert!(foreign.add_player(1, channel::unbounded().0));
        let foreign_result = foreign
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(3)))
            .unwrap();
        assert!(driver.complete(
            &mut session,
            ticket,
            ActionCompletion::InternalResult(foreign_result),
        ));
        assert!(matches!(driver.turn(&mut session), DriverTurn::Waiting));
        session.with_player(1, |ctx| {
            assert_eq!(ctx.player.last_handler_result, old_last);
            assert_eq!(ctx.player.scopes[0].stack.len(), 0);
        });
    }

    #[test]
    fn set_obj_prop_cast_load_uses_cast_ticket_before_write_completion() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::SetObjProp, 5, 0),
            Bytecode::new(OpCode::Ret, 0, 1),
        ]);
        let base_path = url::Url::parse("file:///tmp/").unwrap();
        session
            .with_player(1, |ctx| {
                ctx.player.net_manager.base_path = Some(base_path);
            })
            .unwrap();
        // The handler frame owns the opcode-name table captured at setup;
        // update that owned table rather than mutating the cast's unrelated
        // handler-name snapshot after the frame has been prepared.
        Rc::make_mut(&mut driver.frames[0].ctx.code.names)[5] =
            Symbol::builtin(BuiltInSymbol::FileName);
        let receiver = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::CastLib(1)))
            .unwrap();
        let value = session
            .with_player(1, |ctx| {
                ctx.player.alloc_datum(Datum::String("replacement.cct".to_owned()))
            })
            .unwrap();
        let old_last = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(9)))
            .unwrap();
        session
            .with_player(1, |ctx| {
                ctx.player.last_handler_result = old_last.clone();
                let scope = &mut ctx.player.scopes[0];
                scope.stack.push_value(StackDatum::Ref(receiver));
                scope.stack.push_value(StackDatum::Ref(value));
            })
            .unwrap();

        let (ticket, request) = match driver.turn(&mut session) {
            DriverTurn::Pending(PendingAction::CastLoad { ticket, request }) => (ticket, request),
            _ => panic!("SetObjProp FileName did not produce a CastLoad action"),
        };
        let (_, _, foreign_ticket) = prepared_driver();
        assert!(!driver.complete(
            &mut session,
            foreign_ticket,
            ActionCompletion::InternalResult(DatumRef::Void),
        ));
        assert!(matches!(driver.turn(&mut session), DriverTurn::Waiting));

        assert!(session.apply_cast_load(request.complete(
            "file:///tmp/replacement.cct".to_owned(),
            Err("test setter failure".to_owned()),
        )));
        let completed = session.take_completed_property_casts();
        assert_eq!(completed.len(), 1);
        assert!(completed[0].1.same_identity(&ticket));
        assert!(driver.complete(
            &mut session,
            ticket,
            ActionCompletion::InternalResult(DatumRef::Void),
        ));
        assert!(matches!(
            driver.turn(&mut session),
            DriverTurn::Waiting | DriverTurn::Complete(_)
        ));
        session.with_player(1, |ctx| {
            assert_eq!(ctx.player.last_handler_result, old_last);
            assert_eq!(ctx.player.scopes[0].stack.len(), 0);
            assert_eq!(ctx.player.scopes[0].bytecode_index, 1);
        });
    }

    #[test]
    fn internal_obj_call_v4_uses_stack_name_and_updates_last_result() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::ObjCallV4, 999, 0),
            Bytecode::new(OpCode::Ret, 0, 1),
        ]);
        let handler = session.symbols_mut().intern("unsupportedCall");
        let handler_ref = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Symbol(handler.clone())))
            .unwrap();
        let receiver = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(4)))
            .unwrap();
        let argument = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(5)))
            .unwrap();
        session
            .with_player(1, |ctx| {
                let scope = &mut ctx.player.scopes[0];
                scope.stack.push_value(StackDatum::Ref(receiver.clone()));
                scope.stack.push_value(StackDatum::Ref(argument.clone()));
                scope.stack.push_value(StackDatum::ArgMarker {
                    count: 2,
                    no_ret: false,
                });
                scope.stack.push_value(StackDatum::Ref(handler_ref));
            })
            .unwrap();
        let pending = match driver.turn(&mut session) {
            DriverTurn::Pending(PendingAction::Internal(request)) => request,
            _ => panic!("ObjCallV4 did not produce an internal request"),
        };
        match pending.request {
            InternalVmRequest::ObjectV4 { receiver: actual, name, args } => {
                assert_eq!(actual, receiver);
                assert_eq!(name, handler);
                assert_eq!(args, vec![argument]);
            }
            _ => panic!("ObjCallV4 used the wrong internal request"),
        }
        let result = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(6)))
            .unwrap();
        assert!(driver.complete(
            &mut session,
            pending.ticket,
            ActionCompletion::InternalResult(result),
        ));
        assert!(matches!(driver.turn(&mut session), DriverTurn::Waiting));
        session.with_player(1, |ctx| {
            assert!(matches!(ctx.player.get_datum(&ctx.player.last_handler_result), Datum::Int(6)));
            assert_eq!(ctx.player.scopes[0].stack.len(), 1);
        });
    }

    #[test]
    fn internal_obj_call_v4_js_receiver_wins_over_same_named_global_once() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::ObjCallV4, 999, 0),
            Bytecode::new(OpCode::Ret, 0, 1),
        ]);
        let handler = session.symbols_mut().intern("child");
        let handler_ref = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Symbol(handler.clone())))
            .unwrap();
        let object = Rc::new(RefCell::new(crate::player::js_lingo::value::JsObject::new()));
        let handle = session
            .with_player_js(1, |context, registry| {
                let runtime = Rc::new(RefCell::new(crate::player::js_lingo::interpreter::JsRuntime::new()));
                registry.register_object(1, &context.player.owner, &runtime, &object).unwrap()
            })
            .unwrap();
        let receiver = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::JsObjectRef(handle)))
            .unwrap();
        let argument = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(5)))
            .unwrap();
        session.with_player(1, |ctx| {
            let scope = &mut ctx.player.scopes[0];
            scope.stack.push_value(StackDatum::Ref(receiver.clone()));
            scope.stack.push_value(StackDatum::Ref(argument.clone()));
            scope.stack.push_value(StackDatum::ArgMarker { count: 2, no_ret: false });
            scope.stack.push_value(StackDatum::Ref(handler_ref));
        });

        let pending = match driver.turn(&mut session) {
            DriverTurn::Pending(PendingAction::Internal(request)) => request,
            _ => panic!("ObjCallV4 JS receiver did not produce an internal request"),
        };
        match &pending.request {
            InternalVmRequest::ObjectV4 { receiver: actual, name, args } => {
                assert_eq!(actual, &receiver);
                assert_eq!(name, &handler);
                assert_eq!(args, &vec![argument]);
            }
            _ => panic!("ObjCallV4 JS receiver used the wrong request"),
        }
        assert!(matches!(driver.turn(&mut session), DriverTurn::Waiting));
    }

    #[test]
    fn get_chained_prop_defers_only_owner_bound_receivers_once() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::GetChainedProp, 2, 0),
            Bytecode::new(OpCode::Ret, 0, 1),
        ]);
        Rc::make_mut(&mut driver.frames[0].ctx.code.names)[2] =
            Symbol::builtin(BuiltInSymbol::Value);
        let object = Rc::new(RefCell::new(crate::player::js_lingo::value::JsObject::new()));
        object.borrow_mut().set_own("value", crate::player::js_lingo::value::JsValue::Int(4));
        let handle = session
            .with_player_js(1, |context, registry| {
                let owner = context.player.owner.clone();
                let runtime = Rc::new(RefCell::new(crate::player::js_lingo::interpreter::JsRuntime::new()));
                registry.register_object(1, &owner, &runtime, &object).unwrap()
            })
            .unwrap();
        let receiver = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::JsObjectRef(handle)))
            .unwrap();
        session.with_player(1, |ctx| {
            ctx.player.scopes[0].stack.push_value(StackDatum::Ref(receiver.clone()));
        });

        let pending = match driver.turn(&mut session) {
            DriverTurn::Pending(PendingAction::Internal(request)) => request,
            _ => panic!("GetChainedProp did not defer JS receiver"),
        };
        match pending.request {
            InternalVmRequest::ObjectProperty { receiver: actual, name } => {
                assert_eq!(actual, receiver);
                assert_eq!(name, Symbol::builtin(BuiltInSymbol::Value));
            }
            _ => panic!("GetChainedProp used the wrong owner-bound request"),
        }
        assert!(matches!(driver.phase, DriverPhase::Awaiting { .. }));

        let (mut string_session, mut string_driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::GetChainedProp, 2, 0),
            Bytecode::new(OpCode::Ret, 0, 1),
        ]);
        Rc::make_mut(&mut string_driver.frames[0].ctx.code.names)[2] =
            Symbol::builtin(BuiltInSymbol::Value);
        let string_ref = string_session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::String("g.x".to_owned())))
            .unwrap();
        string_session.with_player(1, |ctx| {
            ctx.player.scopes[0].stack.push_value(StackDatum::Ref(string_ref));
        });
        let pending = match string_driver.turn(&mut string_session) {
            DriverTurn::Pending(PendingAction::Internal(request)) => request,
            _ => panic!("GetChainedProp did not defer String.Value"),
        };
        assert!(matches!(
            pending.request,
            InternalVmRequest::EvaluateValue {
                mode: ValueEvaluationMode::StringPropertyFallback,
                ..
            }
        ));
    }

    #[test]
    fn internal_return_resets_bare_value_and_stops_handler() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::ExtCall, 12, 0),
            Bytecode::new(OpCode::Ret, 0, 1),
        ]);
        let stale = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(88)))
            .unwrap();
        session
            .with_player(1, |ctx| {
                ctx.player.last_handler_result = stale.clone();
                ctx.player.scopes[0].return_value = stale;
                ctx.player.scopes[0].stack.push_value(StackDatum::ArgMarker {
                    count: 0,
                    no_ret: false,
                });
            })
            .unwrap();
        let result = match driver.turn(&mut session) {
            DriverTurn::Complete(result) => result,
            _ => panic!("return did not stop handler"),
        };
        assert_eq!(result.return_value, DatumRef::Void);
    }

    #[test]
    fn internal_tell_rejects_replaced_target_identity() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::TellCall, 13, 0),
            Bytecode::new(OpCode::Ret, 0, 1),
        ]);
        assert!(session.add_player(2, channel::unbounded().0));
        session
            .with_player(1, |ctx| {
                ctx.player.tell_target_stack.push(crate::player::TellTarget {
                    nested_player: Some(2),
                    film_loop: None,
                });
                ctx.player.scopes[0].stack.push_value(StackDatum::ArgMarker {
                    count: 0,
                    no_ret: false,
                });
            })
            .unwrap();
        let pending = match driver.turn(&mut session) {
            DriverTurn::Pending(PendingAction::Internal(request)) => request,
            _ => panic!("TellCall did not produce an internal request"),
        };
        match &pending.request {
            InternalVmRequest::Tell { target: Some(target), .. } => assert_eq!(target.player_id, 2),
            _ => panic!("TellCall did not retain nested target identity"),
        }
        let old = session.remove_player(2);
        assert!(old.is_some());
        assert!(session.add_player(2, channel::unbounded().0));
        let result = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(7)))
            .unwrap();
        assert!(driver.complete(
            &mut session,
            pending.ticket,
            ActionCompletion::InternalResult(result),
        ));
        let error = match driver.turn(&mut session) {
            DriverTurn::Error(error) => error,
            _ => panic!("replaced Tell target was accepted"),
        };
        assert_eq!(error.code, super::super::ScriptErrorCode::InvalidReference);
    }

    #[test]
    fn internal_tell_rejects_reset_target_owner() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::TellCall, 13, 0),
            Bytecode::new(OpCode::Ret, 0, 1),
        ]);
        assert!(session.add_player(2, channel::unbounded().0));
        session
            .with_player(1, |ctx| {
                ctx.player.tell_target_stack.push(crate::player::TellTarget {
                    nested_player: Some(2),
                    film_loop: None,
                });
                ctx.player.scopes[0].stack.push_value(StackDatum::ArgMarker {
                    count: 0,
                    no_ret: false,
                });
            })
            .unwrap();
        let pending = match driver.turn(&mut session) {
            DriverTurn::Pending(PendingAction::Internal(request)) => request,
            _ => panic!("TellCall did not produce an internal request"),
        };
        let target_owner = session
            .with_player(2, |ctx| ctx.player.owner.clone())
            .unwrap();
        target_owner.begin_reset();
        let result = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(7)))
            .unwrap();
        assert!(driver.complete(
            &mut session,
            pending.ticket,
            ActionCompletion::InternalResult(result),
        ));
        let error = match driver.turn(&mut session) {
            DriverTurn::Error(error) => error,
            _ => panic!("reset Tell target was accepted"),
        };
        assert_eq!(error.code, super::super::ScriptErrorCode::InvalidReference);
    }

    #[test]
    fn internal_tell_local_call_updates_scope_and_nested_no_return_ignores_result() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::TellCall, 13, 0),
            Bytecode::new(OpCode::Ret, 0, 1),
        ]);
        let argument = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(31)))
            .unwrap();
        session.with_player(1, |ctx| {
            let scope = &mut ctx.player.scopes[0];
            scope.stack.push_value(StackDatum::Ref(argument.clone()));
            scope.stack.push_value(StackDatum::ArgMarker {
                count: 1,
                no_ret: false,
            });
        });
        let pending = match driver.turn(&mut session) {
            DriverTurn::Pending(PendingAction::Internal(request)) => request,
            _ => panic!("local TellCall did not produce an internal request"),
        };
        assert_eq!(session.with_player(1, |ctx| ctx.player.handler_stack_depth), Some(2));
        match &pending.request {
            InternalVmRequest::MovieAsync(request) => {
                assert_eq!(request.player_id, 1);
                assert!(request.owner.same_identity(
                    &session
                        .with_player(1, |ctx| ctx.player.owner.clone())
                        .expect("Tell owner disappeared")
                ));
                assert_eq!(request.kind, super::super::handlers::movie::MovieAsyncKind::Nothing);
                assert_eq!(&request.args, &vec![argument]);
            }
            _ => panic!("local TellCall retained an unexpected typed request"),
        }
        let local_result = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(32)))
            .unwrap();
        assert!(driver.complete(
            &mut session,
            pending.ticket,
            ActionCompletion::InternalResult(local_result),
        ));
        assert!(matches!(driver.turn(&mut session), DriverTurn::Waiting));
        session.with_player(1, |ctx| {
            assert_eq!(ctx.player.handler_stack_depth, 1);
            assert!(matches!(ctx.player.get_datum(&ctx.player.last_handler_result), Datum::Int(32)));
            assert!(matches!(ctx.player.get_datum(&ctx.player.scopes[0].return_value), Datum::Int(32)));
            assert_eq!(ctx.player.scopes[0].stack.len(), 1);
        });

        let (mut nested_session, mut nested_driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::TellCall, 13, 0),
            Bytecode::new(OpCode::Ret, 0, 1),
        ]);
        assert!(nested_session.add_player(2, channel::unbounded().0));
        nested_session.with_player(1, |ctx| {
            ctx.player.tell_target_stack.push(crate::player::TellTarget {
                nested_player: Some(2),
                film_loop: None,
            });
            ctx.player.scopes[0].stack.push_value(StackDatum::ArgMarker {
                count: 0,
                no_ret: true,
            });
        });
        let nested_pending = match nested_driver.turn(&mut nested_session) {
            DriverTurn::Pending(PendingAction::Internal(request)) => request,
            _ => panic!("nested TellCall did not produce an internal request"),
        };
        assert_eq!(nested_pending.pending_reason.as_deref(), Some("cross-player Tell requires nested executor"));
        assert_eq!(nested_session.with_player(1, |ctx| ctx.player.handler_stack_depth), Some(1));
        let mut foreign = RuntimeSession::new(SymbolOwner { session: 93, generation: 1 });
        assert!(foreign.add_player(1, channel::unbounded().0));
        let foreign_result = foreign
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(33)))
            .unwrap();
        assert!(nested_driver.complete(
            &mut nested_session,
            nested_pending.ticket,
            ActionCompletion::InternalResult(foreign_result),
        ));
        assert!(matches!(nested_driver.turn(&mut nested_session), DriverTurn::Waiting));
        nested_session.with_player(1, |ctx| {
            assert_eq!(ctx.player.scopes[0].stack.len(), 0);
            assert!(matches!(ctx.player.get_datum(&ctx.player.last_handler_result), Datum::Void));
        });
    }

    #[test]
    fn internal_tell_local_return_resets_bare_value_and_stops_handler() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::TellCall, 12, 0),
            Bytecode::new(OpCode::Ret, 0, 1),
        ]);
        let stale = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(88)))
            .unwrap();
        session.with_player(1, |ctx| {
            ctx.player.last_handler_result = stale.clone();
            ctx.player.scopes[0].return_value = stale;
            ctx.player.scopes[0].stack.push_value(StackDatum::ArgMarker {
                count: 0,
                no_ret: false,
            });
        });
        let result = match driver.turn(&mut session) {
            DriverTurn::Complete(result) => result,
            _ => panic!("local Tell return did not stop handler"),
        };
        assert_eq!(result.return_value, DatumRef::Void);
        session.with_player(1, |ctx| {
            assert!(matches!(ctx.player.get_datum(&ctx.player.last_handler_result), Datum::Void));
            assert!(matches!(ctx.player.get_datum(&ctx.player.scopes[0].return_value), Datum::Void));
        });
    }

    #[test]
    fn internal_obj_call_v4_preserves_global_precedence_with_no_return() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::ObjCallV4, 999, 0),
            Bytecode::new(OpCode::Ret, 0, 1),
        ]);
        let handler = session.symbols_mut().intern("child");
        let handler_ref = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Symbol(handler.clone())))
            .unwrap();
        let receiver = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(41)))
            .unwrap();
        let argument = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(42)))
            .unwrap();
        session.with_player(1, |ctx| {
            let scope = &mut ctx.player.scopes[0];
            scope.stack.push_value(StackDatum::Ref(receiver.clone()));
            scope.stack.push_value(StackDatum::Ref(argument.clone()));
            scope.stack.push_value(StackDatum::ArgMarker {
                count: 2,
                no_ret: true,
            });
            scope.stack.push_value(StackDatum::Ref(handler_ref));
        });
        assert!(matches!(driver.turn(&mut session), DriverTurn::Waiting));
        assert_eq!(driver.frames.len(), 2);
        assert_eq!(session.with_player(1, |ctx| ctx.player.handler_stack_depth), Some(2));
        assert_eq!(session.with_player(1, |ctx| ctx.player.scopes[1].args.clone()), Some(vec![receiver, argument]));
        assert!(matches!(driver.turn(&mut session), DriverTurn::Waiting));
        assert_eq!(driver.frames.len(), 1);
        assert_eq!(session.with_player(1, |ctx| ctx.player.handler_stack_depth), Some(1));
        session.with_player(1, |ctx| assert_eq!(ctx.player.scopes[0].stack.len(), 0));
    }

    #[test]
    fn internal_obj_call_v4_rejects_foreign_global_during_precedence_scan() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::ObjCallV4, 999, 0),
            Bytecode::new(OpCode::Ret, 0, 1),
        ]);
        let handler = session.symbols_mut().intern("driverForeignProbe");
        let handler_ref = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Symbol(handler)))
            .unwrap();
        let receiver = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(41)))
            .unwrap();
        let mut foreign = RuntimeSession::new(SymbolOwner { session: 92, generation: 1 });
        assert!(foreign.add_player(1, channel::unbounded().0));
        let foreign_global = foreign
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(99)))
            .unwrap();
        let global_key = session.symbols_mut().intern("driverForeignGlobal");
        session.with_player(1, |ctx| {
            ctx.player.globals.insert(global_key, foreign_global);
            let scope = &mut ctx.player.scopes[0];
            scope.stack.push_value(StackDatum::Ref(receiver));
            scope.stack.push_value(StackDatum::ArgMarker {
                count: 1,
                no_ret: true,
            });
            scope.stack.push_value(StackDatum::Ref(handler_ref));
        });
        let error = match driver.turn(&mut session) {
            DriverTurn::Error(error) => error,
            _ => panic!("foreign global was accepted during precedence scan"),
        };
        assert_eq!(error.code, super::super::ScriptErrorCode::InvalidReference);
    }

    #[test]
    fn internal_global_rejects_foreign_completion_without_consuming_ticket() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::ExtCall, 13, 0),
            Bytecode::new(OpCode::Ret, 0, 1),
        ]);
        session.with_player(1, |ctx| {
            ctx.player.scopes[0].stack.push_value(StackDatum::ArgMarker {
                count: 0,
                no_ret: false,
            });
        });
        let pending = match driver.turn(&mut session) {
            DriverTurn::Pending(PendingAction::Internal(request)) => request,
            _ => panic!("ExtCall did not produce an internal request"),
        };
        let before = session
            .with_player(1, |ctx| {
                (
                    ctx.player.last_handler_result.clone(),
                    ctx.player.scopes[0].return_value.clone(),
                    ctx.player.scopes[0].stack.len(),
                )
            })
            .unwrap();
        let mut foreign = RuntimeSession::new(SymbolOwner { session: 95, generation: 1 });
        assert!(foreign.add_player(1, channel::unbounded().0));
        let foreign_result = foreign
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(101)))
            .unwrap();
        assert!(!driver.complete(
            &mut session,
            pending.ticket.clone(),
            ActionCompletion::InternalResult(foreign_result),
        ));
        let after = session
            .with_player(1, |ctx| {
                (
                    ctx.player.last_handler_result.clone(),
                    ctx.player.scopes[0].return_value.clone(),
                    ctx.player.scopes[0].stack.len(),
                )
            })
            .unwrap();
        assert_eq!(after, before);
        let local_result = session
            .with_player(1, |ctx| ctx.player.alloc_datum(Datum::Int(102)))
            .unwrap();
        assert!(driver.complete(
            &mut session,
            pending.ticket,
            ActionCompletion::InternalResult(local_result),
        ));
        assert!(matches!(driver.turn(&mut session), DriverTurn::Waiting));
    }

    #[test]
    fn internal_decode_rejects_missing_argument_marker_without_action() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::ExtCall, 4, 0),
            Bytecode::new(OpCode::Ret, 0, 1),
        ]);
        session.with_player(1, |ctx| ctx.player.handler_stack_depth = 7);
        assert!(matches!(driver.turn(&mut session), DriverTurn::Error(_)));
        assert_eq!(session.with_player(1, |ctx| ctx.player.handler_stack_depth), Some(7));
        assert!(matches!(driver.phase, DriverPhase::Failed(_)));
    }

    #[test]
    fn internal_ext_call_unknown_handler_errors_without_pending() {
        let (mut session, mut driver) = prepared_running_driver(vec![
            Bytecode::new(OpCode::ExtCall, 4, 0),
            Bytecode::new(OpCode::Ret, 0, 1),
        ]);
        session.with_player(1, |ctx| {
            ctx.player.scopes[0].stack.push_value(StackDatum::ArgMarker {
                count: 0,
                no_ret: false,
            });
        });
        assert!(matches!(driver.turn(&mut session), DriverTurn::Error(_)));
        assert_eq!(session.with_player(1, |ctx| ctx.player.handler_stack_depth), Some(1));
        assert!(matches!(driver.phase, DriverPhase::Failed(_)));
    }
}
