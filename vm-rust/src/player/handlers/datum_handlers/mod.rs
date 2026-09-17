pub mod bitmap;
pub mod cast_lib;
pub mod cast_member;
pub mod cast_member_ref;
pub mod color;
pub mod date;
pub mod int;
pub mod js_object;
pub mod list_handlers;
pub mod math;
pub mod player;
pub mod point;
pub mod prop_list;
pub mod rect;
pub mod script;
pub mod script_instance;
pub mod sound_channel;
pub mod sprite;
pub mod string;
pub mod string_chunk;
pub mod symbol;
pub mod timeout;
pub mod vector;
pub mod void;
pub mod xml;
pub mod flash_object;
pub mod float;
pub mod shockwave3d_object;
pub mod transform3d;
pub mod havok_object;
pub mod physx_object;

use self::date::DateDatumHandlers;
use self::cast_lib::CastLibDatumHandlers;
use self::math::MathDatumHandlers;
use self::vector::VectorDatumHandlers;
use self::void::VoidDatumHandlers;
use self::{
    bitmap::BitmapDatumHandlers, list_handlers::ListDatumHandlers, point::PointDatumHandlers,
    prop_list::PropListDatumHandlers, rect::RectDatumHandlers, sprite::SpriteDatumHandlers,
    string::StringDatumHandlers, string_chunk::StringChunkHandlers, timeout::TimeoutDatumHandlers,
    script::ScriptDatumHandlers, script_instance::ScriptInstanceDatumHandlers,
};

use crate::player::symbols::builtin::BuiltInSymbol;
use crate::player::symbols::symbol::Symbol;
use crate::{
    director::lingo::datum::{Datum, DatumType},
    player::{
        compare::validate_direct_symbol_fields,
        driver::checked_internal_datum,
        session::ExecutionContext,
        xtra::manager::call_instance_handler_explicit,
        DatumRef, ScriptError, ScriptErrorCode,
    },
};

/// Result of attempting a synchronous, session-owned datum method call.
/// Unsupported receivers remain available to the caller's pending path;
/// handled errors are distinct from unsupported dispatch and must propagate.
pub(crate) enum SyncDatumCall {
    Unsupported,
    Pending {
        request: crate::player::driver::InternalVmRequest,
        reason: String,
    },
    Child {
        receiver: Option<crate::player::script_ref::ScriptInstanceRef>,
        handler_ref: crate::player::script::ScriptHandlerRef,
        args: Vec<DatumRef>,
    },
    ChildWithCompletion {
        receiver: Option<crate::player::script_ref::ScriptInstanceRef>,
        handler_ref: crate::player::script::ScriptHandlerRef,
        args: Vec<DatumRef>,
        completion: crate::player::driver::ChildCompletion,
    },
    Handled(Result<DatumRef, ScriptError>),
}

/// Dispatch the datum methods whose implementations already own an explicit
/// execution context. This function never borrows the ambient player and does
/// not manufacture a result for an unsupported receiver.
pub(crate) fn try_call_datum_handler_sync(
    runtime: &mut ExecutionContext<'_>,
    datum: &DatumRef,
    handler_name: Symbol,
    args: &Vec<DatumRef>,
) -> SyncDatumCall {
    let handler_name_display = match runtime.symbols.display(&handler_name) {
        Ok(name) => name.to_owned(),
        Err(_) => {
            return SyncDatumCall::Handled(Err(ScriptError::new_code(
                ScriptErrorCode::InvalidReference,
                "foreign or stale handler symbol".to_string(),
            )))
        }
    };
    // The legacy dispatcher owns recursion protection for this special
    // method. Leave it to that path until its session state is migrated.
    if handler_name == BuiltInSymbol::GetPropertyDescriptionList {
        return SyncDatumCall::Unsupported;
    }

    runtime.player.handler_stack_depth += 1;
    let value = match datum {
        DatumRef::Void => Ok(&Datum::Void),
        _ => runtime
            .player
            .allocator
            .try_get_datum(datum)
            .ok_or_else(|| {
                ScriptError::new_code(
                    ScriptErrorCode::InvalidReference,
                    format!("invalid datum reference {datum}"),
                )
            }),
    };
    let outcome = match value {
        Err(error) => SyncDatumCall::Handled(Err(error)),
        Ok(value) => match validate_direct_symbol_fields(value, runtime.symbols) {
            Err(error) => SyncDatumCall::Handled(Err(error)),
            Ok(()) => match value.type_enum() {
                DatumType::Symbol => SyncDatumCall::Handled(
                    symbol::SymbolDatumHandlers::call(runtime, datum.clone(), handler_name, args),
                ),
                DatumType::ColorRef => SyncDatumCall::Handled(
                    color::ColorDatumHandlers::call(runtime, datum.clone(), handler_name, args),
                ),
                DatumType::DateRef => SyncDatumCall::Handled(
                    DateDatumHandlers::call(runtime, datum.clone(), handler_name, args),
                ),
                DatumType::MathRef => SyncDatumCall::Handled(
                    MathDatumHandlers::call(runtime, datum.clone(), handler_name, args),
                ),
                DatumType::List | DatumType::XmlChildNodes => SyncDatumCall::Handled(
                    ListDatumHandlers::call(runtime, datum, handler_name, args),
                ),
                DatumType::PropList => SyncDatumCall::Handled(
                    PropListDatumHandlers::call(runtime, datum, handler_name, args),
                ),
                DatumType::Void => SyncDatumCall::Handled(
                    VoidDatumHandlers::call(runtime, datum.clone(), handler_name, args),
                ),
                DatumType::Point => SyncDatumCall::Handled(
                    PointDatumHandlers::call(runtime, datum.clone(), handler_name, args),
                ),
                DatumType::Rect => SyncDatumCall::Handled(
                    RectDatumHandlers::call(runtime, datum.clone(), handler_name, args),
                ),
                DatumType::Vector => SyncDatumCall::Handled(
                    VectorDatumHandlers::call(runtime, datum.clone(), handler_name, args),
                ),
                DatumType::String => SyncDatumCall::Handled(
                    StringDatumHandlers::call(runtime, datum, handler_name, args),
                ),
                DatumType::StringChunk => SyncDatumCall::Handled(
                    StringChunkHandlers::call(runtime, datum, handler_name, args),
                ),
                DatumType::ScriptRef => {
                    let name_lower = runtime.symbols.lower(&handler_name).map(str::to_owned);
                    match name_lower {
                        Err(_) => SyncDatumCall::Handled(Err(ScriptError::new_code(
                            ScriptErrorCode::InvalidReference,
                            "foreign or stale script handler symbol".to_owned(),
                        ))),
                        Ok(name) if name == "new" || name == "birth" => {
                            let plan = if name == "new" {
                                ScriptDatumHandlers::prepare_constructor(
                                    runtime.player,
                                    runtime.symbols,
                                    datum,
                                    args,
                                    BuiltInSymbol::New,
                                )
                            } else {
                                ScriptDatumHandlers::prepare_constructor_named(
                                    runtime.player,
                                    runtime.symbols,
                                    datum,
                                    args,
                                    handler_name.clone(),
                                )
                            };
                            match plan {
                                Ok(crate::player::handlers::datum_handlers::script::ScriptConstructorPlan::Complete(result)) =>
                                    SyncDatumCall::Handled(Ok(result)),
                                Ok(crate::player::handlers::datum_handlers::script::ScriptConstructorPlan::Child {
                                    receiver,
                                    handler_ref,
                                    args,
                                    fallback,
                                }) => SyncDatumCall::ChildWithCompletion {
                                    receiver: Some(receiver),
                                    handler_ref,
                                    args,
                                    completion: crate::player::driver::ChildCompletion::ConstructScript { fallback },
                                },
                                Err(error) => SyncDatumCall::Handled(Err(error)),
                            }
                        }
                        // rawnew and handler are reserved by the original
                        // ScriptRef dispatcher. The other static helpers
                        // remain fallback methods so an own or virtual
                        // handler with the same name wins.
                        Ok(name) if name == "rawnew" || name == "handler" => {
                            SyncDatumCall::Handled(ScriptDatumHandlers::call(
                                runtime.player,
                                runtime.symbols,
                                datum,
                                handler_name,
                                args,
                            ))
                        }
                        Ok(name) => {
                            let script_ref = match value {
                                Datum::ScriptRef(script_ref) => script_ref.clone(),
                                _ => unreachable!("ScriptRef type did not contain a ScriptRef datum"),
                            };
                            let script = runtime
                                .player
                                .movie
                                .cast_manager
                                .get_script_by_ref(&script_ref);
                            if let Some(handler_ref) = script
                                .and_then(|script| script.get_own_handler_ref(handler_name.clone()))
                            {
                                let args_result = args.iter().try_for_each(|arg| {
                                    checked_internal_datum(runtime.player, runtime.symbols, arg).map(|_| ())
                                });
                                match args_result {
                                    Err(error) => SyncDatumCall::Handled(Err(error)),
                                    Ok(()) => SyncDatumCall::Child {
                                        receiver: None,
                                        handler_ref,
                                        args: args.clone(),
                                    },
                                }
                            } else {
                                let args_result = args.iter().try_for_each(|arg| {
                                    checked_internal_datum(runtime.player, runtime.symbols, arg).map(|_| ())
                                });
                                match args_result {
                                    Err(error) => SyncDatumCall::Handled(Err(error)),
                                    Ok(()) => match crate::player::virtual_scripts::VirtualScriptRegistry::try_call_handler(
                                        runtime.player,
                                        runtime.symbols,
                                        &script_ref,
                                        None,
                                        handler_name.clone(),
                                        args,
                                    ) {
                                        Ok(Some(result)) => SyncDatumCall::Handled(
                                            checked_internal_datum(runtime.player, runtime.symbols, &result)
                                                .map(|_| result),
                                        ),
                                        Ok(None)
                                            if matches!(
                                                name.as_str(),
                                                "handlers"
                                                    | "getprop"
                                                    | "getpropref"
                                                    | "getaprop"
                                                    | "setprop"
                                                    | "setaprop"
                                            ) => SyncDatumCall::Handled(ScriptDatumHandlers::call(
                                            runtime.player,
                                            runtime.symbols,
                                            datum,
                                            handler_name,
                                            args,
                                        )),
                                        Ok(None) => SyncDatumCall::Handled(Err(ScriptError::new_code(
                                            ScriptErrorCode::HandlerNotFound,
                                            format!("No handler {} for script datum", handler_name_display),
                                        ))),
                                        Err(error) => SyncDatumCall::Handled(Err(error)),
                                    },
                                }
                            }
                        }
                    }
                }
                DatumType::ScriptInstanceRef => match ScriptInstanceDatumHandlers::prepare_call(
                    runtime.player,
                    runtime.symbols,
                    datum,
                    handler_name,
                    args,
                ) {
                    Ok(crate::player::handlers::datum_handlers::script_instance::ScriptInstanceCallPlan::Complete(result)) =>
                        SyncDatumCall::Handled(Ok(result)),
                    Ok(crate::player::handlers::datum_handlers::script_instance::ScriptInstanceCallPlan::Child {
                        receiver,
                        handler_ref,
                        args,
                    }) => SyncDatumCall::Child {
                        receiver: Some(receiver),
                        handler_ref,
                        args,
                    },
                    Err(error) => SyncDatumCall::Handled(Err(error)),
                },
                DatumType::TimeoutRef | DatumType::TimeoutInstance | DatumType::TimeoutFactory => {
                    if handler_name == BuiltInSymbol::New {
                        match TimeoutDatumHandlers::prepare_new(runtime.player, runtime.symbols, datum, args) {
                            Ok(crate::player::handlers::datum_handlers::timeout::TimeoutNewPlan::Complete(result)) => SyncDatumCall::Handled(Ok(result)),
                            Ok(crate::player::handlers::datum_handlers::timeout::TimeoutNewPlan::Child { receiver, handler_ref, args, fallback, timeout_name }) => {
                                SyncDatumCall::ChildWithCompletion {
                                    receiver: Some(receiver),
                                    handler_ref,
                                    args,
                                    completion: crate::player::driver::ChildCompletion::TimeoutNew { timeout_name, fallback },
                                }
                            }
                            Err(error) => SyncDatumCall::Handled(Err(error)),
                        }
                    } else if handler_name == BuiltInSymbol::Forget {
                        match TimeoutDatumHandlers::prepare_forget(runtime.player, runtime.symbols, datum) {
                            Ok(crate::player::handlers::datum_handlers::timeout::TimeoutForgetPlan::Complete(result)) => SyncDatumCall::Handled(Ok(result)),
                            Ok(crate::player::handlers::datum_handlers::timeout::TimeoutForgetPlan::Child { receiver, handler_ref, timeout_name }) => {
                                SyncDatumCall::ChildWithCompletion {
                                    receiver: Some(receiver),
                                    handler_ref,
                                    args: Vec::new(),
                                    completion: crate::player::driver::ChildCompletion::TimeoutForget { timeout_name },
                                }
                            }
                            Err(error) => SyncDatumCall::Handled(Err(error)),
                        }
                    } else {
                        SyncDatumCall::Handled(TimeoutDatumHandlers::call(runtime.player, runtime.symbols, datum, handler_name, args))
                    }
                }
                DatumType::BitmapRef => SyncDatumCall::Handled(
                    BitmapDatumHandlers::call(
                        runtime.player,
                        runtime.symbols,
                        datum,
                        handler_name,
                        args,
                    ),
                ),
                DatumType::CastMemberRef => {
                    // Import/load are asynchronous cast-member boundaries. A
                    // non-Havok `step` remains synchronous; Havok step keeps
                    // its callback-bearing async path.
                    if handler_name == BuiltInSymbol::ImportFileInto
                        || handler_name == BuiltInSymbol::LoadFile
                    {
                        SyncDatumCall::Unsupported
                    } else if handler_name == BuiltInSymbol::Step
                        && matches!(
                            value,
                            Datum::CastMember(member_ref)
                                if runtime
                                    .player
                                    .movie
                                    .cast_manager
                                    .find_member_by_ref(member_ref)
                                    .is_some_and(|member| {
                                        matches!(
                                            member.member_type,
                                            crate::player::cast_member::CastMemberType::HavokPhysics(_)
                                        )
                                    })
                        )
                    {
                        SyncDatumCall::Unsupported
                    } else {
                        SyncDatumCall::Handled(
                            cast_member_ref::CastMemberRefHandlers::call(
                                runtime, datum, handler_name, args,
                            ),
                        )
                    }
                }
                DatumType::CastLibRef => SyncDatumCall::Handled(
                    CastLibDatumHandlers::call(runtime.player, runtime.symbols, datum, handler_name, args),
                ),
                DatumType::XtraInstance => {
                    let Datum::XtraInstance(xtra_name, instance_id) = value else {
                        unreachable!("XtraInstance type did not contain an XtraInstance datum")
                    };
                    let xtra_name = xtra_name.clone();
                    if crate::player::xtra::external::is_registered_for_player(runtime.player, &xtra_name) {
                        match crate::player::xtra::external::prepare_instance_request(
                            runtime.player,
                            runtime.symbols,
                            &xtra_name,
                            *instance_id,
                            &handler_name_display,
                            args,
                        ) {
                            Ok(Some(request)) => SyncDatumCall::Pending {
                                request: crate::player::driver::InternalVmRequest::ExternalXtra(request),
                                reason: format!("external Xtra '{}' instance handler", xtra_name),
                            },
                            Ok(None) => SyncDatumCall::Unsupported,
                            Err(error) => SyncDatumCall::Handled(Err(error)),
                        }
                    } else if matches!(xtra_name.to_ascii_lowercase().as_str(), "multiuser" | "curl" | "fileio" | "xmlparser")
                        && !crate::player::xtra::external::is_registered_for_player(runtime.player, &xtra_name)
                    {
                        if (xtra_name.eq_ignore_ascii_case("multiuser")
                            && (handler_name_display.eq_ignore_ascii_case("connectToNetServer")
                                || handler_name_display.eq_ignore_ascii_case("sendNetMessage")))
                            || (xtra_name.eq_ignore_ascii_case("curl")
                                && handler_name_display.eq_ignore_ascii_case("execAsync"))
                            || xtra_name.eq_ignore_ascii_case("fileio")
                            || xtra_name.eq_ignore_ascii_case("xmlparser")
                        {
                            match crate::player::xtra::manager::call_instance_handler_pending_explicit(
                                runtime.player,
                                runtime.symbols,
                                &xtra_name,
                                datum,
                                &handler_name_display,
                                args,
                            ) {
                                Ok(crate::player::xtra::manager::XtraPendingOrValue::Pending(request)) =>
                                    SyncDatumCall::Pending {
                                        request: crate::player::driver::InternalVmRequest::XtraPending(request),
                                        reason: format!("deferred {} handler on owner-bound {} Xtra instance", handler_name_display, xtra_name),
                                    },
                                Ok(crate::player::xtra::manager::XtraPendingOrValue::Value(value)) =>
                                    SyncDatumCall::Handled(Ok(value)),
                                Err(error) => SyncDatumCall::Handled(Err(error)),
                            }
                        } else {
                            SyncDatumCall::Handled(call_instance_handler_explicit(
                                runtime.player,
                                runtime.symbols,
                                &xtra_name,
                                datum,
                                &handler_name_display,
                                args,
                            ))
                        }
                    } else {
                        SyncDatumCall::Unsupported
                    }
                }
                DatumType::SpriteRef => {
                    let name_lower = runtime
                        .symbols
                        .lower(&handler_name)
                        .map(|name| name.to_owned());
                    match name_lower {
                        Err(_) => SyncDatumCall::Handled(Err(ScriptError::new_code(
                            ScriptErrorCode::InvalidReference,
                            "foreign or stale sprite handler symbol".to_string(),
                        ))),
                        Ok(name)
                            if matches!(
                                name.as_str(),
                                "intersects"
                                    | "getprop"
                                    | "getat"
                                    | "setat"
                                    | "getaprop"
                                    | "setaprop"
                                    | "pointtoword"
                                    | "pointtoline"
                                    | "getpropref"
                                    | "camera"
                                    | "addcamera"
                                    | "removecamera"
                                    | "deletecamera"
                                    | "cameracount"
                            ) => SyncDatumCall::Handled(SpriteDatumHandlers::call(
                                runtime,
                                datum,
                                &handler_name_display,
                                args,
                            )),
                        Ok(_) => SyncDatumCall::Unsupported,
                    }
                }
                DatumType::HavokObjectRef => SyncDatumCall::Handled(
                    crate::player::handlers::datum_handlers::havok_object::HavokObjectDatumHandlers::call(
                        runtime.player,
                        runtime.symbols,
                        datum,
                        &handler_name_display,
                        args,
                    ),
                ),
                DatumType::PhysXObjectRef => SyncDatumCall::Handled(
                    crate::player::handlers::datum_handlers::physx_object::PhysXObjectDatumHandlers::call(
                        runtime.player,
                        runtime.symbols,
                        datum,
                        &handler_name_display,
                        args,
                    ),
                ),
                DatumType::Transform3d => SyncDatumCall::Handled(
                    transform3d::Transform3dDatumHandlers::call(
                        runtime,
                        datum.clone(),
                        handler_name,
                        args,
                    ),
                ),
                _ => SyncDatumCall::Unsupported,
            },
        },
    };
    runtime.player.handler_stack_depth = runtime.player.handler_stack_depth.saturating_sub(1);
    outcome
}

/// Result of the explicit datum dispatcher. A pending value retains the exact
/// receiver, symbol, and arguments so the caller can execute it after this
/// borrow ends; it is never replaced with a synthetic success or failure.
pub(crate) enum DatumDispatch {
    Sync(Result<DatumRef, ScriptError>),
    Child {
        receiver: Option<crate::player::script_ref::ScriptInstanceRef>,
        handler_ref: crate::player::script::ScriptHandlerRef,
        args: Vec<DatumRef>,
        reason: String,
    },
    ChildWithCompletion {
        receiver: Option<crate::player::script_ref::ScriptInstanceRef>,
        handler_ref: crate::player::script::ScriptHandlerRef,
        args: Vec<DatumRef>,
        completion: crate::player::driver::ChildCompletion,
    },
    Pending {
        request: crate::player::driver::InternalVmRequest,
        reason: String,
    },
}

/// Canonical owner-aware datum dispatch entrypoint. Synchronous leaves are
/// executed immediately. Async or callback-bearing leaves remain an owned
/// request for the session executor, which performs host work only after the
/// `ExecutionContext` borrow has ended.
pub(crate) fn player_call_datum_handler(
    runtime: &mut ExecutionContext<'_>,
    obj_ref: &DatumRef,
    handler_name: Symbol,
    args: &Vec<DatumRef>,
) -> DatumDispatch {
    match try_call_datum_handler_sync(runtime, obj_ref, handler_name.clone(), args) {
        SyncDatumCall::Handled(result) => DatumDispatch::Sync(result),
        SyncDatumCall::Pending { request, reason } => DatumDispatch::Pending { request, reason },
        SyncDatumCall::Child { receiver, handler_ref, args } => DatumDispatch::Child {
            receiver,
            handler_ref,
            args,
            reason: "script-instance handler requires an owned child continuation".to_owned(),
        },
        SyncDatumCall::ChildWithCompletion { receiver, handler_ref, args, completion } => DatumDispatch::ChildWithCompletion {
            receiver,
            handler_ref,
            args,
            completion,
        },
        SyncDatumCall::Unsupported => {
            let display = runtime
                .symbols
                .display(&handler_name)
                .map(str::to_owned)
                .unwrap_or_else(|_| "<foreign-handler>".to_owned());
            DatumDispatch::Pending {
                request: crate::player::driver::InternalVmRequest::Object {
                    receiver: obj_ref.clone(),
                    name: handler_name,
                    args: args.clone(),
                },
                reason: format!("datum handler {display} requires deferred dispatch"),
            }
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod sync_dispatch_tests {
    use super::*;
    use async_std::channel;
    use crate::director::lingo::datum::Datum;
    use crate::player::cast_lib::CastMemberRef;
    use crate::player::session::RuntimeSession;
    use crate::player::symbols::symbol_table::SymbolOwner;

    fn local_session() -> RuntimeSession {
        let mut session = RuntimeSession::new(SymbolOwner { session: 71, generation: 1 });
        let (tx, _rx) = channel::unbounded();
        assert!(session.add_player(1, tx));
        session
    }

    #[test]
    fn async_cast_handlers_stay_unsupported_for_sync_driver() {
        let mut session = local_session();
        for handler_name in ["importFileInto", "loadFile"] {
            let handler = session.symbols_mut().intern(handler_name);
            let result = session
                .with_player(1, |mut runtime| {
                    let datum = runtime.player.alloc_datum(Datum::CastMember(CastMemberRef {
                        cast_lib: 1,
                        cast_member: 1,
                    }));
                    let result = try_call_datum_handler_sync(
                        &mut runtime,
                        &datum,
                        handler,
                        &Vec::new(),
                    );
                    assert_eq!(runtime.player.handler_stack_depth, 0);
                    result
                })
                .unwrap();
            assert!(matches!(result, SyncDatumCall::Unsupported));
        }
    }

    #[test]
    fn unknown_sprite_handlers_stay_on_pending_path() {
        let mut session = local_session();
        let handler = session.symbols_mut().intern("pointToChar");
        let result = session
            .with_player(1, |mut runtime| {
                let datum = runtime.player.alloc_datum(Datum::SpriteRef(1));
                let result = try_call_datum_handler_sync(
                    &mut runtime,
                    &datum,
                    handler,
                    &Vec::new(),
                );
                assert_eq!(runtime.player.handler_stack_depth, 0);
                result
            })
            .unwrap();
        assert!(matches!(result, SyncDatumCall::Unsupported));
    }

    #[test]
    fn foreign_handler_and_receiver_are_typed_invalid_references() {
        let mut foreign = RuntimeSession::new(SymbolOwner { session: 72, generation: 1 });
        let foreign_handler = foreign.symbols_mut().intern("foreignOnlyHandler");
        let (tx, _rx) = channel::unbounded();
        assert!(foreign.add_player(1, tx));
        let foreign_datum = foreign
            .with_player(1, |mut runtime| runtime.player.alloc_datum(Datum::Int(1)))
            .unwrap();

        let mut local = local_session();
        let foreign_handler_result = local
            .with_player(1, |mut runtime| {
                let datum = DatumRef::Void;
                let result = try_call_datum_handler_sync(
                    &mut runtime,
                    &datum,
                    foreign_handler,
                    &Vec::new(),
                );
                assert_eq!(runtime.player.handler_stack_depth, 0);
                result
            })
            .unwrap();
        assert!(matches!(
            foreign_handler_result,
            SyncDatumCall::Handled(Err(error)) if error.code == ScriptErrorCode::InvalidReference
        ));

        let local_handler = local.symbols_mut().intern("getAt");
        let foreign_receiver_result = local
            .with_player(1, |mut runtime| {
                let result = try_call_datum_handler_sync(
                    &mut runtime,
                    &foreign_datum,
                    local_handler,
                    &Vec::new(),
                );
                assert_eq!(runtime.player.handler_stack_depth, 0);
                result
            })
            .unwrap();
        assert!(matches!(
            foreign_receiver_result,
            SyncDatumCall::Handled(Err(error)) if error.code == ScriptErrorCode::InvalidReference
        ));
    }
}
