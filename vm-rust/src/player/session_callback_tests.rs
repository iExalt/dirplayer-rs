//! Owner-bound nested callback regression fixtures.
//!
//! These tests deliberately drive the real evaluator child and primary
//! handler tickets.  They do not replace the VM with a mock callback or
//! fabricate a completion capability.

use std::{cell::RefCell, collections::HashMap, future::Future, rc::Rc};

use async_std::channel;
use fxhash::FxHashMap;

use crate::{
    director::{
        chunks::{
            handler::{Bytecode, HandlerDef},
            script::ScriptChunk,
        },
        enums::ScriptType,
        lingo::{datum::Datum, opcode::OpCode},
    },
    player::{
        cast_lib::{CastLib, CastMemberRef},
        cast_member::{CastMember, CastMemberType, FlashMember},
        driver::{ActionCompletion, BroadcastPlan, DriverTurn, GlobalDispatch, InternalVmRequest, PendingAction, PendingCommand},
        eval::{EvalPending, EvalTurn, LingoExpr},
        ownership::OwnerToken,
        script::{Script, ScriptHandlerRef},
        session::{EvalRequestTurn, RuntimeSession},
        symbols::{builtin::BuiltInSymbol, symbol::Symbol, symbol_table::SymbolOwner},
        score::SpriteChannel,
        DatumRef,
    },
};

fn handler(name_id: u16, bytecode_array: Vec<Bytecode>) -> Rc<HandlerDef> {
    Rc::new(HandlerDef {
        name_id,
        bytecode_array,
        bytecode_index_map: FxHashMap::default(),
        argument_name_ids: vec![],
        local_name_ids: vec![],
        global_name_ids: vec![],
        compiled_ir: RefCell::new(None),
    })
}

fn callback_session() -> (
    RuntimeSession,
    ScriptHandlerRef,
    ScriptHandlerRef,
    ScriptHandlerRef,
) {
    let mut session = RuntimeSession::new(SymbolOwner {
        session: 912,
        generation: 1,
    });
    assert!(session.add_player(1, channel::unbounded().0));

    let (primary_name, pass_name, error_name) = session
        .with_player(1, |context| {
            (
                context.symbols.intern("primaryCallback"),
                context.symbols.intern("passCallback"),
                context.symbols.intern("errorCallback"),
            )
        })
        .expect("callback fixture player exists");
    let nothing = Symbol::builtin(BuiltInSymbol::Nothing);
    let pass = Symbol::builtin(BuiltInSymbol::Pass);
    let member_ref = CastMemberRef {
        cast_lib: 1,
        cast_member: 1,
    };

    let suspend_then_pass = vec![
        Bytecode::new(OpCode::PushArgListNoRet, 0, 0),
        Bytecode::new(OpCode::ExtCall, 1, 1),
        Bytecode::new(OpCode::PushArgListNoRet, 0, 2),
        Bytecode::new(OpCode::ExtCall, 2, 3),
        Bytecode::new(OpCode::Ret, 0, 4),
    ];
    let suspend_then_error = vec![
        Bytecode::new(OpCode::PushArgListNoRet, 0, 0),
        Bytecode::new(OpCode::ExtCall, 1, 1),
        Bytecode::new(OpCode::Invalid, 0, 2),
    ];
    let script = Rc::new(Script {
        member_ref: member_ref.clone(),
        name: "callback-fixture".to_owned(),
        chunk: ScriptChunk {
            script_number: 1,
            literals: vec![],
            handlers: vec![],
            property_name_ids: vec![],
            property_defaults: HashMap::new(),
        },
        script_type: ScriptType::Movie,
        handlers: FxHashMap::from_iter([
            (primary_name.clone(), handler(0, suspend_then_pass.clone())),
            (pass_name.clone(), handler(3, suspend_then_pass)),
            (error_name.clone(), handler(4, suspend_then_error)),
        ]),
        handler_names_raw: vec![
            "primaryCallback".to_owned(),
            "nothing".to_owned(),
            "pass".to_owned(),
            "passCallback".to_owned(),
            "errorCallback".to_owned(),
        ],
        handler_names: vec![
            primary_name.clone(),
            nothing.clone(),
            pass.clone(),
            pass_name.clone(),
            error_name.clone(),
        ],
        properties: RefCell::new(FxHashMap::default()),
    });

    session
        .with_player(1, |context| {
            let mut cast = CastLib::test_external(1, 0);
            cast.name_symbols = Rc::from(vec![
                primary_name.clone(),
                nothing,
                pass,
                pass_name.clone(),
                error_name.clone(),
            ]);
            cast.scripts.insert(1, script);
            context.player.movie.cast_manager.casts.push(cast);
        })
        .expect("callback fixture player exists");

    (
        session,
        (member_ref.clone(), primary_name),
        (member_ref.clone(), pass_name),
        (member_ref, error_name),
    )
}

fn owner(session: &mut RuntimeSession) -> OwnerToken {
    session
        .with_player(1, |context| context.player.owner.clone())
        .expect("callback fixture player exists")
}

fn pending_internal(
    session: &mut RuntimeSession,
    initial: DriverTurn,
) -> crate::player::driver::InternalInvocationRequest {
    let mut turn = initial;
    for _ in 0..8 {
        match turn {
            DriverTurn::Pending(PendingAction::Internal(request)) => return request,
            DriverTurn::Waiting => {
                turn = session
                    .turn_handler(1)
                    .expect("primary continuation remains registered while waiting");
            }
            other => panic!(
                "expected a real pending internal action, got {:?}",
                turn_kind(&other)
            ),
        }
    }
    panic!("primary did not reach a pending internal action");
}

fn turn_kind(turn: &DriverTurn) -> &'static str {
    match turn {
        DriverTurn::Waiting => "waiting",
        DriverTurn::Pending(_) => "pending",
        DriverTurn::Complete(_) => "complete",
        DriverTurn::Error(_) => "error",
    }
}

fn pump_child_to_completion(
    session: &mut RuntimeSession,
    id: crate::player::eval::EvalId,
    initial: EvalRequestTurn,
) {
    let mut turn = initial;
    for _ in 0..8 {
        match turn {
            EvalRequestTurn::Child(DriverTurn::Waiting) => {
                turn = session
                    .turn_eval_child(id.clone())
                    .expect("child continuation remains registered while waiting");
            }
            EvalRequestTurn::Evaluator(_) => return,
            EvalRequestTurn::Child(DriverTurn::Pending(_)) => {
                panic!("fixture child issued a second host action")
            }
            EvalRequestTurn::Child(DriverTurn::Complete(_))
            | EvalRequestTurn::Child(DriverTurn::Error(_))
            | EvalRequestTurn::MovieAsync(_)
            | EvalRequestTurn::Flash(_)
            | EvalRequestTurn::ExternalXtra(_)
            | EvalRequestTurn::ExternalXtraLoad(_)
            | EvalRequestTurn::XtraPending(_) => {
                panic!("child completion bypassed evaluator callback delivery")
            }
        }
    }
    panic!("child did not reach a terminal evaluator result");
}

fn pending_child_internal(
    session: &mut RuntimeSession,
    id: &crate::player::eval::EvalId,
    initial: EvalRequestTurn,
) -> crate::player::driver::InternalInvocationRequest {
    let mut turn = initial;
    for _ in 0..8 {
        match turn {
            EvalRequestTurn::Child(DriverTurn::Pending(PendingAction::Internal(request))) => {
                return request;
            }
            EvalRequestTurn::Child(DriverTurn::Waiting) => {
                turn = session
                    .turn_eval_child(id.clone())
                    .expect("child continuation remains registered while waiting");
            }
            EvalRequestTurn::Child(other) => panic!(
                "expected a real pending child action, got {}",
                turn_kind(&other)
            ),
            EvalRequestTurn::Evaluator(_)
            | EvalRequestTurn::MovieAsync(_)
            | EvalRequestTurn::Flash(_)
            | EvalRequestTurn::ExternalXtra(_)
            | EvalRequestTurn::ExternalXtraLoad(_)
            | EvalRequestTurn::XtraPending(_) => {
                panic!("child completed before issuing its internal action")
            }
        }
    }
    panic!("child did not reach a pending internal action");
}

fn finish_primary(session: &mut RuntimeSession) {
    for _ in 0..8 {
        match session.turn_handler(1) {
            Some(DriverTurn::Waiting) => {}
            Some(DriverTurn::Complete(_)) => return,
            Some(DriverTurn::Pending(_)) => panic!("primary unexpectedly issued another action"),
            Some(DriverTurn::Error(error)) => panic!("primary failed: {error:?}"),
            None => panic!("primary driver was removed before completion"),
        }
    }
    panic!("primary did not reach a terminal result");
}

fn waiting_evaluator_action(
    session: &mut RuntimeSession,
) -> (crate::player::eval::EvalId, crate::player::eval::EvalAction) {
    let id = session
        .start_eval(
            1,
            LingoExpr::ObjHandlerCall(
                Box::new(LingoExpr::IntLiteral(1)),
                "deferred".to_owned(),
                vec![],
            ),
        )
        .expect("evaluator should start");
    let action = match session.turn_eval(id.clone()) {
        EvalTurn::Pending {
            request: EvalPending::Object { capability, .. },
        } => capability,
        other => panic!("expected a pending evaluator action, got {}", eval_turn_kind(&other)),
    };
    (id, action)
}

fn eval_turn_kind(turn: &EvalTurn) -> &'static str {
    match turn {
        EvalTurn::Pending { .. } => "pending",
        EvalTurn::Complete(Ok(_)) => "complete-ok",
        EvalTurn::Complete(Err(_)) => "complete-error",
    }
}

#[test]
fn evaluator_external_probe_decline_redispatches_builtin_once() {
    let session = Rc::new(RefCell::new(callback_session().0));
    let (id, capability) = waiting_evaluator_action(&mut session.borrow_mut());
    let owner = owner(&mut session.borrow_mut());
    let object_type = session
        .borrow_mut()
        .with_player(1, |context| context.player.alloc_datum(Datum::String("xml".to_owned())))
        .expect("fixture player exists");
    let fallback_name = Symbol::builtin(BuiltInSymbol::NewObject);
    let request = crate::player::xtra::external::ExternalXtraRequest {
        owner: owner.clone(),
        owner_key: "evaluator-probe".to_owned(),
        xtra_name: String::new(),
        operation: crate::player::xtra::external::ExternalXtraOperation::ProbeStatic {
            handler: "newObject".to_owned(),
            candidates: vec![],
            raw_args: vec![object_type.clone()],
            fallback_name: fallback_name.clone(),
            fallback_args: vec![object_type],
        },
        args: vec![],
    };
    let turn = session.borrow_mut().execute_eval_request(
        id.clone(),
        EvalPending::Global {
            capability: capability.clone(),
            request: InternalVmRequest::ExternalXtra(request),
            reason: None,
            prepared_child: None,
        },
    );
    let EvalRequestTurn::ExternalXtra(request) = turn else {
        panic!("external evaluator request must leave the session borrow first");
    };
    let turn = async_std::task::block_on(crate::player::commands::execute_eval_external_request(
        &session,
        id,
        capability,
        1,
        owner,
        request,
    ));
    let EvalRequestTurn::Evaluator(EvalTurn::Complete(Ok(result))) = turn else {
        panic!("declined probe must redispatch the builtin and complete");
    };
    assert!(matches!(
        session
            .borrow_mut()
            .with_player(1, |context| context.player.get_datum(&result).clone()),
        Some(Datum::XmlRef(_))
    ));
}

#[test]
fn evaluator_unloaded_new_leaves_a_real_owner_load_request() {
    let (mut session, _primary, _pass_handler, _error_handler) = callback_session();
    let (id, capability) = waiting_evaluator_action(&mut session);
    let xtra = session
        .with_player(1, |context| {
            context
                .player
                .alloc_datum(Datum::Xtra("unloaded-fixture".to_owned()))
        })
        .expect("fixture player exists");
    let dispatch = session
        .dispatch_global(1, &Symbol::builtin(BuiltInSymbol::New), &[xtra])
        .expect("new dispatch should classify");
    let turn = session.execute_eval_global_dispatch(
        id,
        capability,
        Symbol::builtin(BuiltInSymbol::New),
        vec![],
        dispatch,
    );
    assert!(matches!(
        turn,
        EvalRequestTurn::ExternalXtraLoad(_)
    ));
}

#[test]
fn evaluator_object_xtra_call_leaves_the_host_request_owned() {
    let session = Rc::new(RefCell::new(callback_session().0));
    let (id, capability) = waiting_evaluator_action(&mut session.borrow_mut());
    let owner = owner(&mut session.borrow_mut());
    let request = crate::player::xtra::external::ExternalXtraRequest {
        owner: owner.clone(),
        owner_key: "object-fixture".to_owned(),
        xtra_name: "mock".to_owned(),
        operation: crate::player::xtra::external::ExternalXtraOperation::Instance {
            instance_id: 1,
            handler: "call".to_owned(),
        },
        args: vec![],
    };
    let turn = session.borrow_mut().execute_eval_request(
        id,
        EvalPending::Object {
            capability,
            request: InternalVmRequest::ExternalXtra(request),
            reason: Some("object Xtra fixture".to_owned()),
        },
    );
    assert!(matches!(turn, EvalRequestTurn::ExternalXtra(_)));
}

#[test]
fn declined_external_probe_lowers_async_builtin_to_typed_movie_request() {
    let session = Rc::new(RefCell::new(callback_session().0));
    let (id, capability) = waiting_evaluator_action(&mut session.borrow_mut());
    let owner = owner(&mut session.borrow_mut());
    let request = crate::player::xtra::external::ExternalXtraRequest {
        owner: owner.clone(),
        owner_key: "async-probe-fixture".to_owned(),
        xtra_name: String::new(),
        operation: crate::player::xtra::external::ExternalXtraOperation::ProbeStatic {
            handler: "go".to_owned(),
            candidates: vec![],
            raw_args: vec![],
            fallback_name: Symbol::builtin(BuiltInSymbol::Go),
            fallback_args: vec![],
        },
        args: vec![],
    };
    let turn = async_std::task::block_on(
        crate::player::commands::execute_eval_external_request_with(
            &session,
            id,
            capability,
            1,
            owner,
            request,
            |_request| Ok(None),
        ),
    );
    assert!(matches!(
        turn,
        EvalRequestTurn::MovieAsync(crate::player::handlers::movie::MovieAsyncRequest {
            kind: crate::player::handlers::movie::MovieAsyncKind::Go,
            ..
        })
    ));
}

#[test]
fn reset_retires_evaluator_child_driver_and_callback_scope() {
    let (mut session, _primary, pass_handler, _error_handler) = callback_session();
    let (id, capability) = waiting_evaluator_action(&mut session);
    let child_turn = session.execute_eval_global_dispatch(
        id.clone(),
        capability,
        Symbol::builtin(BuiltInSymbol::Call),
        vec![],
        GlobalDispatch::ChildPrepared {
            receiver: None,
            handler_ref: pass_handler,
            args: vec![],
        },
    );
    assert!(matches!(child_turn, EvalRequestTurn::Child(_)));
    assert!(session.eval_drivers.contains_key(&id));
    let old_owner = owner(&mut session);
    let new_owner = session.reset_player_owned(1, &old_owner).expect("reset owner");
    assert!(!session.eval_drivers.contains_key(&id));
    assert!(new_owner.is_arena_live());
}

#[test]
fn evaluator_external_host_can_reenter_and_reset_rejects_late_completion() {
    let session = Rc::new(RefCell::new(callback_session().0));
    let (id, capability) = waiting_evaluator_action(&mut session.borrow_mut());
    let owner = owner(&mut session.borrow_mut());
    let request = crate::player::xtra::external::ExternalXtraRequest {
        owner: owner.clone(),
        owner_key: "reentry-owner".to_owned(),
        xtra_name: "mock".to_owned(),
        operation: crate::player::xtra::external::ExternalXtraOperation::Static {
            handler: "reenter".to_owned(),
        },
        args: vec![],
    };
    let host_session = session.clone();
    let host_owner = owner.clone();
    let turn = async_std::task::block_on(
        crate::player::commands::execute_eval_external_request_with(
            &session,
            id,
            capability,
            1,
            owner,
            request,
            move |_request| {
                assert!(host_session.try_borrow_mut().is_ok());
                host_session
                    .borrow_mut()
                    .reset_player_owned(1, &host_owner)
                    .expect("host callback can retire its captured owner");
                Ok(Some(crate::player::xtra::external::ExternalXtraResponse {
                    xtra_name: "mock".to_owned(),
                    bytes: vec![],
                }))
            },
        ),
    );
    assert!(matches!(
        turn,
        EvalRequestTurn::Evaluator(EvalTurn::Complete(Err(_)))
    ));
}

#[test]
fn evaluator_declined_host_reset_rejects_fallback_and_consumes_action() {
    let session = Rc::new(RefCell::new(callback_session().0));
    let (id, capability) = waiting_evaluator_action(&mut session.borrow_mut());
    let owner = owner(&mut session.borrow_mut());
    let request = crate::player::xtra::external::ExternalXtraRequest {
        owner: owner.clone(),
        owner_key: "declined-reset".to_owned(),
        xtra_name: String::new(),
        operation: crate::player::xtra::external::ExternalXtraOperation::ProbeStatic {
            handler: "declined".to_owned(),
            candidates: vec![],
            raw_args: vec![],
            fallback_name: Symbol::builtin(BuiltInSymbol::NewObject),
            fallback_args: vec![],
        },
        args: vec![],
    };
    let host_session = session.clone();
    let host_owner = owner.clone();
    let turn = async_std::task::block_on(
        crate::player::commands::execute_eval_external_request_with(
            &session,
            id.clone(),
            capability.clone(),
            1,
            owner.clone(),
            request,
            move |_request| {
                host_session
                    .borrow_mut()
                    .reset_player_owned(1, &host_owner)
                    .expect("declined host can retire its captured owner");
                Ok(None)
            },
        ),
    );
    assert!(matches!(turn, EvalRequestTurn::Evaluator(EvalTurn::Complete(Err(_)))));
    assert!(session
        .borrow_mut()
        .eval_action_anchor(&id, &capability)
        .is_none());
}

#[test]
fn evaluator_claimed_host_error_consumes_action() {
    let session = Rc::new(RefCell::new(callback_session().0));
    let (id, capability) = waiting_evaluator_action(&mut session.borrow_mut());
    let owner = owner(&mut session.borrow_mut());
    let request = crate::player::xtra::external::ExternalXtraRequest {
        owner: owner.clone(),
        owner_key: "claimed-error".to_owned(),
        xtra_name: "mock".to_owned(),
        operation: crate::player::xtra::external::ExternalXtraOperation::Static {
            handler: "fails".to_owned(),
        },
        args: vec![],
    };
    let turn = async_std::task::block_on(
        crate::player::commands::execute_eval_external_request_with(
            &session,
            id.clone(),
            capability.clone(),
            1,
            owner,
            request,
            |_request| Err(crate::player::ScriptError::new("host failure".to_owned())),
        ),
    );
    assert!(matches!(turn, EvalRequestTurn::Evaluator(EvalTurn::Complete(Err(_)))));
    assert!(session
        .borrow_mut()
        .eval_action_anchor(&id, &capability)
        .is_none());
}

#[test]
fn evaluator_external_probe_child_sequence_preserves_initial_result() {
    let (mut session, _primary, _pass_handler, _error_handler) = callback_session();
    let (id, capability) = waiting_evaluator_action(&mut session);
    let initial = session
        .with_player(1, |context| context.player.alloc_datum(Datum::Int(77)))
        .expect("fixture player exists");
    let turn = session.execute_eval_global_dispatch(
        id,
        capability,
        Symbol::builtin(BuiltInSymbol::SendAllSprites),
        vec![],
        GlobalDispatch::ChildSequence {
            plan: BroadcastPlan {
                calls: vec![],
                fallback: vec![],
                initial_return: initial,
                handled: true,
                continue_on_error: true,
            },
        },
    );
    let EvalRequestTurn::Evaluator(EvalTurn::Complete(Ok(result))) = turn else {
        panic!("empty child sequence must complete through the evaluator");
    };
    assert!(matches!(
        session.with_player(1, |context| context.player.get_datum(&result).clone()),
        Some(Datum::Int(77))
    ));
}

#[test]
fn evaluator_child_prepared_route_runs_real_child_to_completion() {
    let (mut session, _primary, pass_handler, _error_handler) = callback_session();
    let (id, capability) = waiting_evaluator_action(&mut session);
    let child_turn = session.execute_eval_global_dispatch(
        id.clone(),
        capability,
        Symbol::builtin(BuiltInSymbol::Call),
        vec![],
        GlobalDispatch::ChildPrepared {
            receiver: None,
            handler_ref: pass_handler,
            args: vec![],
        },
    );
    let initial_request = pending_child_internal(&mut session, &id, child_turn);
    let mut turn = EvalRequestTurn::Child(DriverTurn::Pending(PendingAction::Internal(initial_request)));
    for _ in 0..8 {
        turn = match turn {
            EvalRequestTurn::Child(DriverTurn::Pending(PendingAction::Internal(request))) => session
                .complete_eval_child_action(
                    id.clone(),
                    request.ticket,
                    ActionCompletion::InternalResult(DatumRef::Void),
                )
                .expect("child action should be accepted"),
            EvalRequestTurn::Child(DriverTurn::Waiting) => session
                .turn_eval_child(id.clone())
                .expect("child continuation should remain registered"),
            EvalRequestTurn::Child(DriverTurn::Pending(_)) => {
                panic!("prepared child issued an unexpected second host action")
            }
            EvalRequestTurn::Evaluator(result) => {
                assert!(matches!(result, EvalTurn::Complete(Ok(_))));
                return;
            }
            EvalRequestTurn::Child(DriverTurn::Complete(_))
            | EvalRequestTurn::Child(DriverTurn::Error(_))
            | EvalRequestTurn::MovieAsync(_)
            | EvalRequestTurn::Flash(_)
            | EvalRequestTurn::ExternalXtra(_)
            | EvalRequestTurn::ExternalXtraLoad(_)
            | EvalRequestTurn::XtraPending(_) => {
                panic!("prepared child bypassed evaluator completion")
            }
        };
    }
    panic!("prepared evaluator child did not complete");
}

#[test]
fn nested_callback_preserves_primary_driver_and_pass_result() {
    let (mut session, primary, pass_handler, _error_handler) = callback_session();
    assert!(session
        .start_handler(1, None, primary, &[], true)
        .unwrap()
        .is_none());
    let primary_turn = session
        .turn_handler(1)
        .expect("primary driver is installed");
    let primary_request = pending_internal(&mut session, primary_turn);
    let primary_ticket = primary_request.ticket.clone();
    assert!(session.action_details(&primary_ticket).is_some());

    let captured = owner(&mut session);
    let (child_id, child_turn, callback) = session
        .start_owned_script_callback(1, captured.clone(), None, pass_handler, vec![], true)
        .expect("nested callback starts under the suspended primary");
    let child_action = pending_child_internal(&mut session, &child_id, child_turn);
    let child_turn = session
        .complete_eval_child_action(
            child_id.clone(),
            child_action.ticket.clone(),
            ActionCompletion::InternalResult(DatumRef::Void),
        )
        .expect("child completion is accepted for the captured owner");
    pump_child_to_completion(&mut session, child_id, child_turn);
    let child_result = async_std::task::block_on(callback.recv())
        .expect("child completion sender remains connected");
    match &child_result {
        Ok(scope) => assert!(scope.passed, "child pass result was not preserved"),
        Err(_) => panic!("child pass callback returned an error"),
    }
    assert!(
        session.action_details(&primary_ticket).is_some(),
        "primary ticket was lost while child ran"
    );

    assert!(session.complete_handler_action(
        primary_ticket,
        ActionCompletion::InternalResult(DatumRef::Void)
    ));
    finish_primary(&mut session);
}

#[test]
fn nested_callback_runs_actual_movie_async_child_through_command_pump() {
    let (session, _primary, pass_handler, _error_handler) = callback_session();
    let session = Rc::new(RefCell::new(session));
    let captured = owner(&mut session.borrow_mut());
    let (child_id, child_turn, callback) = session
        .borrow_mut()
        .start_owned_script_callback(1, captured.clone(), None, pass_handler, vec![], true)
        .expect("nested callback starts through the child evaluator");
    let child_action = pending_child_internal(&mut session.borrow_mut(), &child_id, child_turn);
    match &child_action.request {
        InternalVmRequest::MovieAsync(request) => assert_eq!(
            request.kind,
            crate::player::handlers::movie::MovieAsyncKind::Nothing
        ),
        _ => panic!("child bytecode did not produce the MovieAsync request"),
    }
    let child_ticket = child_action.ticket.clone();
    session.borrow_mut().retain_pending_command(PendingCommand {
        player_id: 1,
        owner: captured.clone(),
        action: Some(PendingAction::Internal(child_action)),
        started: false,
        ticket: Some(child_ticket.clone()),
        completer: None,
        event_sender: None,
        score_continuation: None,
        child_completion: None,
        eval_child: Some(child_id),
        eval_sender: None,
    });
    let result = async_std::task::block_on(async_std::future::timeout(
        std::time::Duration::from_secs(2),
        async {
            let (_pump, callback_result) = futures::join!(
                crate::player::commands::drive_pending_owner(&session, 1, &captured),
                callback.recv(),
            );
            callback_result
        },
    ))
        .unwrap_or_else(|_| {
            let state = session.borrow();
            panic!(
                "actual MovieAsync child owner pump timed out: retained={}, ready={}",
                state.has_pending_commands(1),
                state.has_ready_pending_commands(1),
            )
        })
        .expect("actual MovieAsync child completion sender remains connected")
        .expect("actual MovieAsync child should complete");
    assert!(result.passed, "actual MovieAsync child lost ScopeResult::passed");
    assert!(
        session.borrow().action_details(&child_ticket).is_none(),
        "completed MovieAsync child ticket must be retired"
    );
}

#[test]
fn nested_callback_external_load_wait_is_cancelled_by_owner_reset() {
    let (session, _primary, pass_handler, _error_handler) = callback_session();
    let session = Rc::new(RefCell::new(session));
    let captured = owner(&mut session.borrow_mut());
    let mut load_request = session
        .borrow_mut()
        .with_player(1, |context| {
            let request = context.player.xtra_manager_state.external.begin_load(
                captured.clone(),
                "callback-load".to_owned(),
                "CallbackXtra",
            )?;
            context.player.xtra_manager_state.external.attach_continuation(
                &request,
                crate::player::xtra::external::ExternalXtraContinuation::Create {
                    xtra_name: "CallbackXtra".to_owned(),
                    args: Vec::new(),
                },
            )?;
            Ok::<_, crate::player::ScriptError>(request)
        })
        .expect("external load request should be prepared")
        .expect("callback fixture player exists");
    // The test completes the owner-local waiter directly; no browser load
    // notification is needed for this reset/cancellation path.
    load_request.notify_host = false;
    let (child_id, child_turn, callback) = session
        .borrow_mut()
        .start_owned_script_callback(1, captured.clone(), None, pass_handler, vec![], true)
        .expect("nested callback starts through the child evaluator");
    let child_action = pending_child_internal(&mut session.borrow_mut(), &child_id, child_turn);
    let mut invocation = child_action;
    invocation.request = InternalVmRequest::ExternalXtraLoad(load_request);
    let child_ticket = invocation.ticket.clone();
    session.borrow_mut().retain_pending_command(PendingCommand {
        player_id: 1,
        owner: captured.clone(),
        action: Some(PendingAction::Internal(invocation)),
        started: false,
        ticket: Some(child_ticket),
        completer: None,
        event_sender: None,
        score_continuation: None,
        child_completion: None,
        eval_child: Some(child_id),
        eval_sender: None,
    });
    let mut pump = Box::pin(crate::player::commands::execute_pending_commands(
        &session,
        1,
        &captured,
    ));
    let waker = futures::task::noop_waker();
    let mut context = std::task::Context::from_waker(&waker);
    assert!(matches!(
        pump.as_mut().poll(&mut context),
        std::task::Poll::Pending
    ));
    let replacement = session
        .borrow_mut()
        .reset_player_owned(1, &captured)
        .expect("owner reset should retire the waiting child");
    assert!(replacement.is_arena_live());
    let (_turns, _wake) = async_std::task::block_on(pump);
    let result = async_std::task::block_on(callback.recv())
        .expect("reset should complete the callback channel");
    let error = match result {
        Ok(_) => panic!("reset must reject the stale external load child"),
        Err(error) => error,
    };
    assert_eq!(error.code, crate::player::ScriptErrorCode::Abort);
    assert!(session
        .borrow_mut()
        .with_player(1, |context| context.player.owner.same_identity(&replacement))
        .unwrap_or(false));
}

#[test]
fn evaluator_sync_xtra_close_completes_receiver_and_drains_teardown() {
    let mut session = RuntimeSession::new(SymbolOwner { session: 913, generation: 1 });
    assert!(session.add_player(1, channel::unbounded().0));
    let (receiver_datum, close_symbol) = session
        .with_player(1, |context| {
            let instance_id = context
                .player
                .xtra_manager_state
                .multiuser
                .create_instance(&Vec::new());
            let receiver = context
                .player
                .alloc_datum(Datum::XtraInstance("multiuser".to_owned(), instance_id));
            (receiver, context.symbols.intern("close"))
    })
        .expect("evaluator fixture player exists");
    let request = InternalVmRequest::Object {
        receiver: receiver_datum,
        name: close_symbol,
        args: Vec::new(),
    };
    let session = Rc::new(RefCell::new(session));
    let (_id, _action, result_receiver) = session
        .borrow_mut()
        .start_eval_request(1, request)
        .expect("close request should create an evaluator continuation");

    // This is the evaluator's production pump boundary. The handler closes
    // the Xtra while the session borrow is active; the route registered by
    // take_pending_eval_request_for must deliver the synchronous success to
    // the caller before the pump drains released host resources.
    let progressed = async_std::task::block_on(crate::player::commands::pump_pending_eval_requests(
        &session,
        1,
    ));
    assert!(progressed);
    assert!(
        async_std::task::block_on(result_receiver.recv())
            .expect("synchronous evaluator should complete its receiver")
            .is_ok()
    );
    assert!(!session.borrow().has_pending_eval_requests(1));
    assert_eq!(session.borrow().pending_host_teardown_count(), 0);
}

#[test]
fn evaluator_flash_request_uses_owned_transport_and_consumes_native_failure() {
    let mut session = RuntimeSession::new(SymbolOwner { session: 914, generation: 1 });
    assert!(session.add_player(1, channel::unbounded().0));
    let owner = session
        .with_player(1, |context| context.player.owner.clone())
        .expect("Flash fixture player exists");
    let swf = include_bytes!("../../tests/fixtures/flash_initial_access.swf").to_vec();
    session
        .with_player(1, |context| {
            let mut cast = CastLib::test_external(1, 0);
            cast.insert_member(
                1,
                CastMember::new(
                    1,
                    CastMemberType::Flash(FlashMember {
                        data: swf,
                        reg_point: (0, 0),
                        flash_info: None,
                    }),
                ),
                context.symbols,
            );
            context.player.movie.cast_manager.casts.push(cast);
            let mut channel = SpriteChannel::new(1);
            channel.sprite.member = Some(CastMemberRef { cast_lib: 1, cast_member: 1 });
            context.player.movie.score.channels = vec![SpriteChannel::new(0), channel];
        })
        .expect("Flash fixture cast/member must be installed");
    let request = InternalVmRequest::Flash(
        crate::player::handlers::datum_handlers::flash_object::FlashRequest {
            player_id: 1,
            owner: owner.clone(),
            sprite_num: 1,
            expected_generation: None,
            path: "_root.fixture".to_owned(),
            operation: crate::player::handlers::datum_handlers::flash_object::FlashOperation::BindGet {
                return_mode: crate::player::handlers::datum_handlers::flash_object::FlashReturnMode::Scalar,
            },
            cast_lib: 1,
            cast_member: 1,
        },
    );
    let session = Rc::new(RefCell::new(session));
    let (_id, _action, receiver) = session
        .borrow_mut()
        .start_eval_request(1, request)
        .expect("Flash request should create an evaluator continuation");
    let progressed = async_std::task::block_on(crate::player::commands::pump_pending_eval_requests(
        &session,
        1,
    ));
    assert!(progressed, "the owner pump must consume the Flash request");
    let result = async_std::task::block_on(receiver.recv())
        .expect("owned Flash transport must complete its evaluator receiver");
    let error = result.expect_err("native Flash transport must fail explicitly");
    assert!(error.message.contains("Flash host is unavailable on native"));
    assert!(!session.borrow().has_pending_eval_requests(1));
}

#[test]
fn requeued_real_eval_action_retires_old_route_before_final_receiver_result() {
    let mut session = RuntimeSession::new(SymbolOwner { session: 915, generation: 1 });
    assert!(session.add_player(1, channel::unbounded().0));
    let owner = session
        .with_player(1, |context| context.player.owner.clone())
        .expect("Flash fixture player exists");
    let session = Rc::new(RefCell::new(session));
    let expression = LingoExpr::Add(
        Box::new(LingoExpr::ObjHandlerCall(
            Box::new(LingoExpr::ListLiteral(vec![LingoExpr::IntLiteral(1)])),
            "count".to_owned(),
            vec![],
        )),
        Box::new(LingoExpr::ObjHandlerCall(
            Box::new(LingoExpr::ListLiteral(vec![
                LingoExpr::IntLiteral(1),
                LingoExpr::IntLiteral(2),
            ])),
            "count".to_owned(),
            vec![],
        )),
    );
    let id = session
        .borrow_mut()
        .start_eval(1, expression)
        .expect("multi-step evaluator should start");
    let first = match session.borrow_mut().turn_eval(id.clone()) {
        EvalTurn::Pending { request } => request,
        other => panic!("evaluator should issue its first request, got {}", eval_turn_kind(&other)),
    };
    let first_action = match &first {
        EvalPending::Global { capability, .. }
        | EvalPending::Object { capability, .. }
        | EvalPending::SetProperty { capability, .. } => capability.clone(),
    };
    let (sender, receiver) = channel::bounded(1);
    session
        .borrow_mut()
        .retain_pending_eval_request(id.clone(), 1, owner, first, sender);
    let mut seen_actions = vec![first_action];
    let mut completed = false;
    for _ in 0..8 {
        if !session.borrow().has_pending_eval_requests(1) {
            completed = true;
            break;
        }
        assert!(async_std::task::block_on(
            crate::player::commands::pump_pending_eval_requests(&session, 1)
        ));
        assert!(session.borrow().inflight_eval_routes.is_empty());
        if let Some(action) = session
            .borrow()
            .pending_eval_requests
            .iter()
            .find(|pending| pending.player_id == 1)
            .map(|pending| pending.action.clone())
        {
            if !seen_actions.iter().any(|seen| seen == &action) {
                seen_actions.push(action);
            }
        }
    }
    let result = async_std::task::block_on(receiver.recv())
        .expect("multi-step evaluator should complete its receiver");
    let result = result.expect("multi-step evaluator should complete successfully");
    assert!(session
        .borrow_mut()
        .with_player(1, |context| {
            matches!(context.player.allocator.try_get_datum(&result), Some(Datum::Int(3)))
        })
        .unwrap_or(false));
    assert!(seen_actions.len() >= 2, "continuation must issue a new action after the first result");
    assert!(completed);
    assert!(session.borrow().inflight_eval_routes.is_empty());
}

#[test]
fn reset_cancels_sibling_detached_eval_receivers_once() {
    let mut session = RuntimeSession::new(SymbolOwner { session: 916, generation: 1 });
    assert!(session.add_player(1, channel::unbounded().0));
    let owner = session
        .with_player(1, |context| context.player.owner.clone())
        .expect("Flash fixture player exists");
    let flash_request = |owner: &OwnerToken| {
        InternalVmRequest::Flash(
            crate::player::handlers::datum_handlers::flash_object::FlashRequest {
                player_id: 1,
                owner: owner.clone(),
                sprite_num: 1,
                expected_generation: None,
                path: "_root.fixture".to_owned(),
                operation: crate::player::handlers::datum_handlers::flash_object::FlashOperation::BindGet {
                    return_mode: crate::player::handlers::datum_handlers::flash_object::FlashReturnMode::Scalar,
                },
                cast_lib: 0,
                cast_member: 0,
            },
        )
    };
    let session = Rc::new(RefCell::new(session));
    let (id_a, _action_a, receiver_a) = session
        .borrow_mut()
        .start_eval_request(1, flash_request(&owner))
        .expect("first detached request should be retained");
    let (id_b, _action_b, receiver_b) = session
        .borrow_mut()
        .start_eval_request(1, flash_request(&owner))
        .expect("sibling detached request should be retained");
    assert_ne!(id_a, id_b);
    let _ = session
        .borrow_mut()
        .take_pending_eval_request_for(1)
        .expect("first request should register an inflight route");
    let _ = session
        .borrow_mut()
        .take_pending_eval_request_for(1)
        .expect("sibling request should register an inflight route");
    assert_eq!(session.borrow().inflight_eval_routes.len(), 2);

    let replacement = session
        .borrow_mut()
        .reset_player_owned(1, &owner)
        .expect("reset should retire both detached routes");
    assert!(replacement.is_arena_live());
    for receiver in [receiver_a, receiver_b] {
        let error = async_std::task::block_on(receiver.recv())
            .expect("reset should complete every evaluator receiver")
            .expect_err("reset must reject stale evaluator work");
        assert_eq!(error.code, crate::player::ScriptErrorCode::Abort);
        assert!(receiver.try_recv().is_err(), "reset must not complete a receiver twice");
    }
    assert!(session.borrow().inflight_eval_routes.is_empty());
}

#[test]
fn nested_callback_preserves_non_abort_error() {
    let (mut session, primary, _pass_handler, error_handler) = callback_session();
    assert!(session
        .start_handler(1, None, primary, &[], true)
        .unwrap()
        .is_none());
    let primary_turn = session
        .turn_handler(1)
        .expect("primary driver is installed");
    let primary_request = pending_internal(&mut session, primary_turn);
    let primary_ticket = primary_request.ticket.clone();
    let captured = owner(&mut session);
    let (child_id, child_turn, callback) = session
        .start_owned_script_callback(1, captured, None, error_handler, vec![], true)
        .expect("error callback starts through the child evaluator");
    let child_action = pending_child_internal(&mut session, &child_id, child_turn);
    let child_turn = session
        .complete_eval_child_action(
            child_id.clone(),
            child_action.ticket,
            ActionCompletion::InternalResult(DatumRef::Void),
        )
        .expect("error child completion is accepted for the captured owner");
    pump_child_to_completion(&mut session, child_id, child_turn);
    let error = match async_std::task::block_on(callback.recv())
        .expect("error callback sender remains connected")
    {
        Ok(_) => panic!("Invalid bytecode unexpectedly completed successfully"),
        Err(error) => error,
    };
    assert_eq!(error.code, crate::player::ScriptErrorCode::Generic);
    assert_eq!(error.message, "No handler for opcode invalid (0x00)");
    assert!(
        session.action_details(&primary_ticket).is_some(),
        "non-Abort child error removed the suspended primary"
    );
    assert!(session.complete_handler_action(
        primary_ticket,
        ActionCompletion::InternalResult(DatumRef::Void)
    ));
    finish_primary(&mut session);
}

#[test]
fn reset_rejects_late_child_and_primary_completions() {
    let (mut session, primary, pass_handler, _error_handler) = callback_session();
    assert!(session
        .start_handler(1, None, primary, &[], true)
        .unwrap()
        .is_none());
    let primary_turn = session
        .turn_handler(1)
        .expect("primary driver is installed");
    let primary_request = pending_internal(&mut session, primary_turn);
    let primary_ticket = primary_request.ticket;
    let captured = owner(&mut session);
    let (child_id, child_turn, _callback) = session
        .start_owned_script_callback(1, captured.clone(), None, pass_handler, vec![], true)
        .expect("nested callback starts");
    let child_ticket = pending_child_internal(&mut session, &child_id, child_turn).ticket;

    let replacement = session
        .reset_player_owned(1, &captured)
        .expect("captured owner can reset the player");
    let replacement_state = session
        .with_player(1, |context| {
            (
                context.player.scope_count,
                context.player.handler_stack_depth,
            )
        })
        .expect("replacement player exists");
    assert!(session
        .complete_eval_child_action(
            child_id,
            child_ticket,
            ActionCompletion::InternalResult(DatumRef::Void),
        )
        .is_none());
    assert!(!session.complete_handler_action(
        primary_ticket,
        ActionCompletion::InternalResult(DatumRef::Void),
    ));
    assert_eq!(
        session.with_player(1, |context| {
            (
                context.player.owner.same_identity(&replacement),
                context.player.scope_count,
                context.player.handler_stack_depth,
            )
        }),
        Some((true, replacement_state.0, replacement_state.1))
    );
}
