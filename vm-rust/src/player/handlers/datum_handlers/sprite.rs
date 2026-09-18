use log::warn;
use wasm_bindgen::prelude::*;

use crate::js_api::JsApi;
use crate::director::lingo::datum::Datum;

use crate::player::symbols::builtin::BuiltInSymbol;
use crate::player::symbols::symbol::Symbol;
use crate::player::{
    allocator::ScriptInstanceAllocatorTrait,
    compare::validate_direct_symbol_fields,
    cast_member::CastMemberType,
    font::{get_text_index_at_pos, DrawTextParams},
    player_handle_scope_return,
    owner_key_string, reserve_player_mut, reserve_player_ref,
    script::{script_get_prop, script_set_prop},
    script_ref::ScriptInstanceRef, DatumRef, DirPlayer, ScriptError, ScriptErrorCode,
    session::ExecutionContext,
    score::{get_concrete_sprite_rect, get_sprite_rect_in_context},
    symbols::symbol_table::SymbolTable,
};

use super::script_instance::ScriptInstanceUtils;

/// The sprite's Shockwave3D camera list as one ordered vec (index 1 first).
///
/// Director gives every 3D sprite a camera list that ALREADY contains the cast
/// member's default camera at index 1 — `cameraCount()` is 1 before a script has
/// touched it, and `addCamera(cam)` with no index appends *after* that camera. We
/// store the list as `w3d_camera` (index 1) + `w3d_cameras` (2+), and both start
/// empty, so seed index 1 with the member's own default view.
///
/// Without the seed, SuperSonic RC's
///     tSprite.addCamera(p3d.camera("overlays"))
/// made the in-scene overlay camera index 1 — it replaced the game world's default
/// camera instead of layering over it, and since it clears on render the sprite
/// came out black.
fn sprite_camera_list(
    player: &DirPlayer,
    symbols: &SymbolTable,
    sprite_num: i16,
) -> Result<Vec<crate::player::sprite::SpriteCamera>, ScriptError> {
    let Some(sprite) = player.movie.score.get_sprite(sprite_num) else {
        return Ok(vec![]);
    };
    let mut cams: Vec<_> = sprite.w3d_camera.iter().cloned().collect();
    cams.extend(sprite.w3d_cameras.iter().cloned());
    if cams.is_empty() {
        // Same default the renderer picks when no camera is active: the member's
        // "DefaultView", else its first view node.
        use crate::director::chunks::w3d::types::W3dNodeType;
        let name = if let Some(scene) = sprite
            .member
            .as_ref()
            .and_then(|m| player.movie.cast_manager.find_member_by_ref(m))
            .and_then(|m| m.member_type.as_shockwave3d())
            .and_then(|o| o.parsed_scene.as_ref())
        {
            let mut first_view = None;
            let mut default_view = None;
            for node in &scene.nodes {
                if node.node_type != W3dNodeType::View {
                    continue;
                }
                if first_view.is_none() {
                    first_view = Some(node);
                }
                if symbols
                    .lower(&node.name)
                    .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
                    .eq_ignore_ascii_case("DefaultView")
                {
                    default_view = Some(node);
                    break;
                }
            }
            match default_view.or(first_view) {
                Some(node) => symbols
                    .display(&node.name)
                    .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
                    .to_owned(),
                None => "DefaultView".to_string(),
            }
        } else {
            "DefaultView".to_string()
        };
        cams.push(crate::player::sprite::SpriteCamera { member: None, name });
    }
    Ok(cams)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_foreign_view_name_returns_invalid_reference() {
        let mut session = crate::player::session::RuntimeSession::new(
            crate::player::symbols::symbol_table::SymbolOwner {
                session: 903,
                generation: 1,
            },
        );
        let (tx, _rx) = async_std::channel::unbounded();
        assert!(session.add_player(1, tx));
        let mut foreign_symbols = crate::player::symbols::symbol_table::SymbolTable::with_owner(
            crate::player::symbols::symbol_table::SymbolOwner {
                session: 904,
                generation: 1,
            },
        );
        let foreign_name = foreign_symbols.intern("foreignView");

        let result = session
            .with_player(1, |mut runtime| {
                runtime
                    .player
                    .movie
                    .score
                    .channels
                    .push(crate::player::score::SpriteChannel::new(0));
                runtime
                    .player
                    .movie
                    .score
                    .channels
                    .push(crate::player::score::SpriteChannel::new(1));
                let mut scene = crate::director::chunks::w3d::types::W3dScene::default();
                scene.nodes.push(crate::director::chunks::w3d::types::W3dNode {
                    name: foreign_name,
                    node_type: crate::director::chunks::w3d::types::W3dNodeType::View,
                    ..Default::default()
                });
                let info = crate::director::enums::Shockwave3dInfo {
                    loops: false,
                    duration: 0,
                    direct_to_stage: false,
                    animation_enabled: false,
                    preload: false,
                    reg_point: (0, 0),
                    default_rect: (0, 0, 640, 480),
                    camera_position: None,
                    camera_rotation: None,
                    bg_color: None,
                    ambient_color: None,
                };
                let scene = std::rc::Rc::new(scene);
                let w3d = crate::player::cast_member::Shockwave3dMember {
                    info: info.clone(),
                    w3d_data: Vec::new(),
                    source_scene: Some(scene.clone()),
                    parsed_scene: Some(scene.clone()),
                    runtime_state: crate::player::cast_member::Shockwave3dRuntimeState::from_info(
                        &info,
                        Some(scene.as_ref()),
                    ),
                    converted_from_text: false,
                    text3d_state: None,
                    text3d_source: None,
                };
                runtime
                    .player
                    .movie
                    .cast_manager
                    .casts
                    .push(crate::player::cast_lib::CastLib::test_external(1, 0));
                runtime.player.movie.cast_manager.casts[0].members.insert(
                    1,
                    crate::player::cast_member::CastMember::new(
                        1,
                        crate::player::cast_member::CastMemberType::Shockwave3d(w3d),
                    ),
                );
                runtime.player.movie.score.channels[1].sprite.member = Some(
                    crate::player::cast_lib::CastMemberRef {
                        cast_lib: 1,
                        cast_member: 1,
                    },
                );
                let receiver = runtime.player.alloc_datum(Datum::SpriteRef(1));
                SpriteDatumHandlers::call(
                    &mut runtime,
                    &receiver,
                    "cameracount",
                    &vec![],
                )
            })
            .unwrap();

        assert_eq!(result.unwrap_err().code, ScriptErrorCode::InvalidReference);
    }

    fn flash_callback_fixture() -> (
        crate::player::session::RuntimeSessionHandle,
        crate::player::ownership::OwnerToken,
        DatumRef,
        Symbol,
    ) {
        let session = crate::player::session::RuntimeSession::new(
            crate::player::symbols::symbol_table::SymbolOwner {
                session: 905,
                generation: 1,
            },
        )
        .into_handle();
        assert!(session.borrow_mut().add_player(1, async_std::channel::unbounded().0));
        let (owner, receiver, handler) = session
            .borrow_mut()
            .with_player(1, |context| {
                let mut cast = crate::player::cast_lib::CastLib::test_external(1, 0);
                cast.insert_member(
                    1,
                    crate::player::cast_member::CastMember::new(
                        1,
                        crate::player::cast_member::CastMemberType::Flash(
                            crate::player::cast_member::FlashMember {
                                data: include_bytes!(
                                    "../../../../tests/fixtures/flash_lingo_callback_probe.swf"
                                )
                                .to_vec(),
                                reg_point: (0, 0),
                                flash_info: None,
                            },
                        ),
                    ),
                    context.symbols,
                );
                context.player.movie.cast_manager.casts.push(cast);
                let mut channel = crate::player::score::SpriteChannel::new(1);
                channel.sprite.member = Some(crate::player::cast_lib::CastMemberRef {
                    cast_lib: 1,
                    cast_member: 1,
                });
                context.player.movie.score.channels = vec![
                    crate::player::score::SpriteChannel::new(0),
                    channel,
                    crate::player::score::SpriteChannel::new(2),
                ];
                context.player.movie.score.sprite_spans = vec![
                    crate::player::score::ScoreSpriteSpan {
                        channel_number: 1,
                        start_frame: 1,
                        end_frame: 1,
                        scripts: vec![],
                    },
                ];
                context.player.movie.score.frame_count = Some(1);
                context.player.movie.score.invalidate_span_channel_cache();
                let owner = context.player.owner.clone();
                let receiver = context.player.alloc_datum(Datum::SpriteRef(1));
                let handler = context.symbols.intern("setCallback");
                (owner, receiver, handler)
            })
            .expect("callback fixture player exists");
        (session, owner, receiver, handler)
    }

    fn callback_request(
        owner: &crate::player::ownership::OwnerToken,
        receiver: DatumRef,
        handler: Symbol,
        args: Vec<DatumRef>,
    ) -> crate::player::driver::SpriteAsyncRequest {
        crate::player::driver::SpriteAsyncRequest {
            player_id: 1,
            owner: owner.clone(),
            receiver,
            sprite_num: 1,
            handler,
            args,
        }
    }

    fn callback_args(
        session: &crate::player::session::RuntimeSessionHandle,
        object: crate::director::lingo::datum::FlashObjectRef,
    ) -> Vec<DatumRef> {
        session
            .borrow_mut()
            .with_player(1, |context| {
                let handler = context.symbols.intern("onFlashCallback");
                vec![
                    context.player.alloc_datum(Datum::FlashObjectRef(object)),
                    context.player.alloc_datum(Datum::String("callback".to_owned())),
                    context.player.alloc_datum(Datum::Symbol(handler)),
                ]
            })
            .expect("callback fixture player exists")
    }

    #[test]
    fn set_callback_short_arity_returns_void_without_generation_or_host_work() {
        let (session, owner, receiver, handler) = flash_callback_fixture();
        let result = async_std::task::block_on(SpriteDatumHandlers::execute_async_request(
            session.clone(),
            callback_request(&owner, receiver, handler, Vec::new()),
        ))
        .expect("short setCallback must return successfully");
        let observed = session
            .borrow_mut()
            .with_player(1, |context| context.player.get_datum(&result).clone())
            .expect("callback fixture player exists");
        assert!(matches!(observed, Datum::Void));
        let (generation, actions) = session
            .borrow_mut()
            .with_player(1, |context| {
                (context.player.flash_instance_generation(1), context.player.take_flash_host_actions())
            })
            .expect("callback fixture player exists");
        assert_eq!(generation, None);
        assert!(actions.is_empty(), "short setCallback must not prepare host work");
    }

    #[test]
    fn synchronous_set_callback_rejects_before_local_connection_mutation() {
        let (session, _owner, receiver, _handler) = flash_callback_fixture();
        session
            .borrow_mut()
            .with_player(1, |mut runtime| {
                let callback_handler = runtime.symbols.intern("onFlashCallback");
                let args = vec![
                    runtime.player.alloc_datum(Datum::String("_root".to_owned())),
                    runtime.player.alloc_datum(Datum::String("callback".to_owned())),
                    runtime.player.alloc_datum(Datum::Symbol(callback_handler)),
                ];
                let before = runtime.player.flash_lc_callbacks.len();
                let error = SpriteDatumHandlers::call(&mut runtime, &receiver, "setCallback", &args)
                    .expect_err("synchronous setCallback must require the owner-bound async path");
                assert_eq!(error.message, "setCallback requires the owner-bound async path");
                assert_eq!(runtime.player.flash_lc_callbacks.len(), before);
            })
            .expect("callback fixture player exists");
    }

    #[test]
    fn set_callback_string_reaches_explicit_native_host_unsupported_boundary() {
        let (session, owner, receiver, handler) = flash_callback_fixture();
        let args = session
            .borrow_mut()
            .with_player(1, |context| {
                let callback_handler = context.symbols.intern("onFlashCallback");
                vec![
                    context.player.alloc_datum(Datum::String("_root".to_owned())),
                    context.player.alloc_datum(Datum::String("callback".to_owned())),
                    context.player.alloc_datum(Datum::Symbol(callback_handler)),
                ]
            })
            .expect("callback fixture player exists");
        let error = async_std::task::block_on(SpriteDatumHandlers::execute_async_request(
            session,
            callback_request(&owner, receiver, handler, args),
        ))
        .expect_err("native setCallback registration must reject explicitly");
        assert_eq!(error.message, "Flash host is unavailable on native");
    }

    #[test]
    fn evaluator_set_callback_string_reaches_native_unsupported_boundary() {
        let (session, owner, receiver, handler) = flash_callback_fixture();
        let args = session
            .borrow_mut()
            .with_player(1, |context| {
                let callback_handler = context.symbols.intern("onFlashCallback");
                vec![
                    context.player.alloc_datum(Datum::String("_root".to_owned())),
                    context.player.alloc_datum(Datum::String("callback".to_owned())),
                    context.player.alloc_datum(Datum::Symbol(callback_handler)),
                ]
            })
            .expect("callback fixture player exists");
        let request = crate::player::driver::InternalVmRequest::SpriteAsync(callback_request(
            &owner,
            receiver,
            handler,
            args,
        ));
        let (_id, _action, result_receiver) = session
            .borrow_mut()
            .start_eval_request(1, request)
            .expect("sprite callback evaluator request should start");
        let pending = session
            .borrow_mut()
            .take_pending_eval_request_for(1)
            .expect("sprite callback evaluator request should be retained");
        async_std::task::block_on(crate::player::commands::pump_admitted_eval_request(
            &session, pending,
        ));
        let error = async_std::task::block_on(result_receiver.recv())
            .expect("evaluator result should be delivered")
            .expect_err("native sprite callback must reject explicitly");
        assert_eq!(error.message, "Flash host is unavailable on native");
    }

    #[test]
    fn owned_sprite_call_preparation_preserves_path_and_json_string_only() {
        let (session, _owner, _receiver, _handler) = flash_callback_fixture();
        let request = session
            .borrow_mut()
            .with_player(1, |context| {
                let path = context.player.alloc_datum(Datum::String(
                    "_root.dirplayerInvokeCallbackProbe".to_owned(),
                ));
                let args_xml = context
                    .player
                    .alloc_datum(Datum::String("[\"payload\"]".to_owned()));
                let trailing_number = context.player.alloc_datum(Datum::Int(7));
                let trailing_object = context.player.alloc_datum(Datum::String("ignored".to_owned()));
                let args = vec![path, args_xml, trailing_number, trailing_object];
                let (path, args_xml) = prepare_sprite_call_inputs(
                    context.player,
                    context.symbols,
                    &args,
                )
                .expect("sprite call inputs should prepare");
                let generation = context
                    .player
                    .reserve_flash_instance_generation(1)
                    .expect("generation should reserve");
                crate::player::handlers::datum_handlers::flash_object::FlashObjectDatumHandlers::prepare_sprite_call(
                    context.player,
                    1,
                    generation,
                    path,
                    args_xml,
                    1,
                    1,
                )
            })
            .expect("callback fixture player exists")
            .expect("owned sprite call should prepare");
        assert_eq!(request.path, "_root.dirplayerInvokeCallbackProbe");
        assert_eq!(request.expected_generation, Some(1));
        match request.operation {
            crate::player::handlers::datum_handlers::flash_object::FlashOperation::Call { args_xml } => {
                assert_eq!(args_xml, "[\"payload\"]");
            }
            _ => panic!("sprite call must prepare a Flash Call operation"),
        }
    }

    #[test]
    fn sprite_call_function_reaches_owned_native_unsupported_boundary() {
        let (session, owner, receiver, _handler) = flash_callback_fixture();
        let handler = session
            .borrow_mut()
            .with_player(1, |context| context.symbols.intern("callFunction"))
            .expect("callback fixture player exists");
        let args = session
            .borrow_mut()
            .with_player(1, |context| {
                vec![
                    context.player.alloc_datum(Datum::String(
                        "_root.dirplayerInvokeCallbackProbe".to_owned(),
                    )),
                    context.player.alloc_datum(Datum::String("[]".to_owned())),
                    context.player.alloc_datum(Datum::Int(7)),
                ]
            })
            .expect("callback fixture player exists");
        let error = async_std::task::block_on(SpriteDatumHandlers::execute_async_request(
            session,
            callback_request(&owner, receiver, handler, args),
        ))
        .expect_err("native sprite callFunction must reject explicitly");
        assert_eq!(error.message, "Flash host is unavailable on native");
    }

    #[test]
    fn set_callback_rejects_foreign_stale_and_different_sprite_objects_before_host_work() {
        let cases = [
            ("foreign", "foreign:905:1:1", 1_u64, 1_i32),
            ("stale", "905:1:1", 1_u64, 1_i32),
            ("different-sprite", "905:1:1", 1_u64, 2_i32),
        ];
        for (label, object_owner, generation, sprite_num) in cases {
            let (session, owner, receiver, handler) = flash_callback_fixture();
            if label == "stale" {
                session
                    .borrow_mut()
                    .with_player(1, |context| {
                        let current = context.player.reserve_flash_instance_generation(1).expect("generation");
                        assert_eq!(current, generation);
                        assert!(context.player.invalidate_flash_instance_generation(1, current));
                    })
                    .expect("callback fixture player exists");
            }
            let object = crate::director::lingo::datum::FlashObjectRef::from_path_with_sprite(
                "_root",
                1,
                1,
                sprite_num,
            )
            .with_binding(object_owner, generation);
            let args = callback_args(&session, object);
            let error = async_std::task::block_on(SpriteDatumHandlers::execute_async_request(
                session.clone(),
                callback_request(&owner, receiver, handler, args),
            ))
            .expect_err("invalid callback object must be rejected");
            assert_eq!(error.code, ScriptErrorCode::InvalidReference, "{label} error");
            assert!(
                error.message.contains("owner")
                    || error.message.contains("generation")
                    || error.message.contains("sprite"),
                "{label} error should identify the invalid binding: {}",
                error.message
            );
            let actions = session
                .borrow_mut()
                .with_player(1, |context| context.player.take_flash_host_actions())
                .expect("callback fixture player exists");
            assert!(actions.is_empty(), "{label} callback rejection emitted host work");
        }
    }

    #[test]
    fn sprite_set_variable_checks_consumed_arguments_before_host_work() {
        let (session, owner, receiver, _callback_handler) = flash_callback_fixture();
        let handler = session
            .borrow_mut()
            .with_player(1, |context| context.symbols.intern("setVariable"))
            .expect("callback fixture player exists");
        let path_only = session
            .borrow_mut()
            .with_player(1, |context| {
                vec![context.player.alloc_datum(Datum::String("value".to_owned()))]
            })
            .expect("callback fixture player exists");
        let error = async_std::task::block_on(SpriteDatumHandlers::execute_async_request(
            session.clone(),
            callback_request(&owner, receiver, handler, path_only),
        ))
        .expect_err("missing setVariable value must be rejected");
        assert!(error.message.contains("path and value"));
        let actions = session
            .borrow_mut()
            .with_player(1, |context| context.player.take_flash_host_actions())
            .expect("callback fixture player exists");
        assert!(actions.is_empty(), "argument rejection must precede host work");
    }

    #[test]
    fn sprite_set_variable_ignores_valid_trailing_arguments_before_owned_native_failure() {
        let (session, owner, receiver, _callback_handler) = flash_callback_fixture();
        let handler = session
            .borrow_mut()
            .with_player(1, |context| context.symbols.intern("setVariable"))
            .expect("callback fixture player exists");
        let args = session
            .borrow_mut()
            .with_player(1, |context| {
                vec![
                    context.player.alloc_datum(Datum::String("value".to_owned())),
                    context.player.alloc_datum(Datum::Int(7)),
                    context.player.alloc_datum(Datum::String("ignored".to_owned())),
                ]
            })
            .expect("callback fixture player exists");
        let error = async_std::task::block_on(SpriteDatumHandlers::execute_async_request(
            session,
            callback_request(&owner, receiver, handler, args),
        ))
        .expect_err("native owned setVariable must reject at host boundary");
        assert_eq!(error.message, "Flash host is unavailable on native");
    }

    #[test]
    fn sprite_get_variable_default_and_object_mode_use_owned_native_boundary() {
        for object_mode in [false, true] {
            let (session, owner, receiver, _callback_handler) = flash_callback_fixture();
            let handler = session
                .borrow_mut()
                .with_player(1, |context| context.symbols.intern("getVariable"))
                .expect("callback fixture player exists");
            let args = session
                .borrow_mut()
                .with_player(1, |context| {
                    if object_mode {
                        vec![
                            context.player.alloc_datum(Datum::String(
                                "_root.callbackProbeString".to_owned(),
                            )),
                            context.player.alloc_datum(Datum::Int(0)),
                        ]
                    } else {
                        Vec::new()
                    }
                })
                .expect("callback fixture player exists");
            let error = async_std::task::block_on(SpriteDatumHandlers::execute_async_request(
                session,
                callback_request(&owner, receiver, handler, args),
            ))
            .expect_err("native owned getVariable must reject explicitly");
            assert_eq!(error.message, "Flash host is unavailable on native");
        }
    }

    #[test]
    fn sprite_get_variable_absent_object_binds_and_reuses_pending_generation() {
        let (session, owner, receiver, _callback_handler) = flash_callback_fixture();
        session
            .borrow_mut()
            .with_player(1, |context| {
                context.player.movie.score.channels[1].sprite.member = None;
            })
            .expect("callback fixture player exists");
        let handler = session
            .borrow_mut()
            .with_player(1, |context| context.symbols.intern("getVariable"))
            .expect("callback fixture player exists");
        let make_args = |session: &crate::player::session::RuntimeSessionHandle| {
            session
                .borrow_mut()
                .with_player(1, |context| {
                    vec![
                        context.player.alloc_datum(Datum::String("early".to_owned())),
                        context.player.alloc_datum(Datum::Int(0)),
                    ]
                })
                .expect("callback fixture player exists")
        };
        let first = async_std::task::block_on(SpriteDatumHandlers::execute_async_request(
            session.clone(),
            callback_request(&owner, receiver.clone(), handler.clone(), make_args(&session)),
        ))
        .expect("absent object access should return a bound handle");
        let second = async_std::task::block_on(SpriteDatumHandlers::execute_async_request(
            session.clone(),
            callback_request(&owner, receiver, handler, make_args(&session)),
        ))
        .expect("repeated absent object access should reuse its binding");
        let (first_object, second_object, generation) = session
            .borrow_mut()
            .with_player(1, |context| {
                let first = context.player.get_datum(&first).as_flash_object().cloned();
                let second = context.player.get_datum(&second).as_flash_object().cloned();
                (first, second, context.player.flash_instance_generation(1))
            })
            .expect("callback fixture player exists");
        let first_object = first_object.expect("first result must be a Flash object");
        let second_object = second_object.expect("second result must be a Flash object");
        assert_eq!(first_object.instance_id, 1);
        assert_eq!(first_object.cast_lib, 0);
        assert_eq!(first_object.cast_member, 0);
        assert_eq!(first_object.owner_key, Some(crate::player::owner_key_string(&owner)));
        assert_eq!(first_object.instance_generation, generation);
        assert_eq!(second_object.instance_generation, generation);
    }

    #[test]
    fn sprite_get_variable_unresolved_object_binds_exact_pair_without_host_work() {
        let (session, owner, receiver, _callback_handler) = flash_callback_fixture();
        session
            .borrow_mut()
            .with_player(1, |context| {
                context.player.movie.score.channels[1].sprite.member =
                    Some(crate::player::cast_lib::CastMemberRef {
                        cast_lib: 1,
                        cast_member: 99,
                    });
            })
            .expect("callback fixture player exists");
        let handler = session
            .borrow_mut()
            .with_player(1, |context| context.symbols.intern("getVariable"))
            .expect("callback fixture player exists");
        let args = session
            .borrow_mut()
            .with_player(1, |context| {
                vec![
                    context.player.alloc_datum(Datum::String("early".to_owned())),
                    context.player.alloc_datum(Datum::Int(0)),
                ]
            })
            .expect("callback fixture player exists");
        let result = async_std::task::block_on(SpriteDatumHandlers::execute_async_request(
            session.clone(),
            callback_request(&owner, receiver, handler, args),
        ))
        .expect("unresolved object access should return a bound handle");
        let object = session
            .borrow_mut()
            .with_player(1, |context| context.player.get_datum(&result).as_flash_object().cloned())
            .expect("callback fixture player exists")
            .expect("result must be a Flash object");
        assert_eq!((object.cast_lib, object.cast_member), (1, 99));
        assert_eq!(object.owner_key, Some(crate::player::owner_key_string(&owner)));
        assert!(object.instance_generation.is_some());
    }
}

/// Write an ordered camera list back to the sprite's primary + extras split.
fn set_sprite_camera_list(
    player: &mut DirPlayer,
    sprite_num: i16,
    mut cams: Vec<crate::player::sprite::SpriteCamera>,
) {
    let sprite = player.movie.score.get_sprite_mut(sprite_num);
    sprite.w3d_camera = if cams.is_empty() { None } else { Some(cams.remove(0)) };
    sprite.w3d_cameras = cams;
}

fn checked_datum<'a>(runtime: &'a ExecutionContext<'_>, datum_ref: &DatumRef) -> Result<&'a Datum, ScriptError> {
    checked_player_datum(runtime.player, runtime.symbols, datum_ref)
}

fn checked_player_datum<'a>(player: &'a DirPlayer, symbols: &SymbolTable, datum_ref: &DatumRef) -> Result<&'a Datum, ScriptError> {
    let datum = match datum_ref {
        DatumRef::Void => &Datum::Void,
        _ => player.allocator.try_get_datum(datum_ref).ok_or_else(|| {
            ScriptError::new_code(ScriptErrorCode::InvalidReference, format!("invalid datum reference {datum_ref}"))
        })?,
    };
    validate_direct_symbol_fields(datum, symbols)?;
    Ok(datum)
}

fn prepare_sprite_call_inputs(
    player: &DirPlayer,
    symbols: &SymbolTable,
    args: &[DatumRef],
) -> Result<(String, String), ScriptError> {
    let path = args
        .first()
        .ok_or_else(|| ScriptError::new("callFunction requires a path".to_owned()))
        .and_then(|arg| checked_player_datum(player, symbols, arg))?
        .string_value(symbols)
        .map(|path| root_flash_path(&path))?;
    let args_xml = args
        .get(1)
        .map(|arg| checked_player_datum(player, symbols, arg).and_then(|datum| datum.string_value(symbols)))
        .transpose()?
        .unwrap_or_default();
    Ok((path, args_xml))
}

fn checked_script_instance(player: &DirPlayer, instance_ref: &ScriptInstanceRef) -> Result<(), ScriptError> {
    if player.allocator.get_script_instance_opt(instance_ref).is_none() {
        return Err(ScriptError::new_code(
            ScriptErrorCode::InvalidReference,
            "foreign or stale ScriptInstanceRef".to_string(),
        ));
    }
    Ok(())
}

fn checked_sprite_num(runtime: &mut ExecutionContext<'_>, datum: &DatumRef) -> Result<i16, ScriptError> {
    checked_datum(runtime, datum)?;
    runtime.with_player(|player| player.get_datum(datum).to_sprite_ref())
}

// JS bridge names use the `dirplayer_` prefix so this fork's globals don't
// collide with stock Ruffle if both are loaded on the same page (e.g. via a
// browser extension). Matching JS-side definitions live in
// src/services/flashPlayerManager.ts::initFlashBridge.
#[wasm_bindgen]
extern "C" {
    // All Lingo Flash bridge calls now take `sprite_num` because each Flash
    // sprite has its own Ruffle instance (per-sprite refactor — multiple
    // sprites sharing one cast member each get an independent player).
    /// `frame_or_label` is the raw Lingo argument as a string. Director's
    /// `sprite(N).gotoFrame(...)` accepts either a frame number (e.g. `3`)
    /// or a label (e.g. `"warm0"`); the JS side parses and routes to
    /// the appropriate Ruffle call. Always passing a string here lets us
    /// preserve labels — `.int_value()` on the Rust side silently parses
    /// non-numeric strings to `0`, and `GotoFrame(0, true)` is treated as
    /// a stop, which froze mello's Fire/Marshmello SWFs.
    #[wasm_bindgen(js_name = "dirplayer_ruffleGoToFrame")]
    fn ruffle_goto_frame(sprite_num: i32, frame_or_label: &str);
    #[wasm_bindgen(js_name = "dirplayer_ruffleStop")]
    fn ruffle_stop(sprite_num: i32);
    #[wasm_bindgen(js_name = "dirplayer_rufflePlayOwned")]
    fn ruffle_play_owned(owner_key: &str, sprite_num: i32);
    #[wasm_bindgen(js_name = "dirplayer_ruffleRewind")]
    fn ruffle_rewind(sprite_num: i32);
    #[wasm_bindgen(js_name = "dirplayer_ruffleCallFrame")]
    fn ruffle_call_frame(sprite_num: i32, frame: i32);
    #[wasm_bindgen(js_name = "dirplayer_ruffleCallFunction", catch)]
    fn ruffle_call_function(sprite_num: i32, path: &str, args_xml: &str) -> Result<JsValue, JsValue>;
    /// Classify what's under a sprite-local point: 0 = #background,
    /// 1 = #normal, 2 = #button, 3 = #editText (Director Flash hitTest values).
    #[wasm_bindgen(js_name = "dirplayer_ruffleHitTest")]
    fn ruffle_hit_test(sprite_num: i32, x: f64, y: f64) -> i32;
    #[wasm_bindgen(js_name = "dirplayer_ruffleGetFlashProperty", catch)]
    fn ruffle_get_flash_property(sprite_num: i32, target: &str, prop_num: i32) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(js_name = "dirplayer_ruffleSetFlashProperty")]
    fn ruffle_set_flash_property(sprite_num: i32, target: &str, prop_num: i32, value: &str);
}

/// Root a bare Flash variable/function path.
///
/// The Ruffle fork's GetVariable/SetVariable/CallFunction run on a
/// `from_nothing` AVM1 activation where an unqualified timeline name does NOT
/// resolve — only explicit `_root`/`_level0`/`_global`/`this`/slash paths do
/// (confirmed empirically: `objMain` → undefined, `_root.objMain` → the
/// object). Director's own Flash Asset resolves such names relative to
/// `_level0`, so prefix a bare name with `_level0.` (JS translateLevel0 maps
/// `_level0` → `_root` before it reaches Ruffle). Already-qualified paths are
/// left untouched.
pub(crate) fn root_flash_path(path: &str) -> String {
    if path.is_empty()
        || path.starts_with("_level0")
        || path.starts_with("_root")
        || path.starts_with("_global")
        || path.starts_with("this")
        || path.starts_with('/')
    {
        path.to_string()
    } else {
        format!("_level0.{}", path)
    }
}

pub struct SpriteDatumHandlers {}

pub struct SpriteDatumUtils {}

impl SpriteDatumUtils {
    pub fn get_script_instance_ids(
        datum: &DatumRef,
        player: &DirPlayer,
    ) -> Result<Vec<ScriptInstanceRef>, ScriptError> {
        let sprite_num = player.get_datum(datum).to_sprite_ref()?;
        let sprite = player.movie.score.get_sprite(sprite_num);
        if sprite.is_none() {
            return Ok(vec![]);
        }
        let sprite = sprite.unwrap();
        let instances = &sprite.script_instance_list;
        Ok(instances.clone())
    }

    /// Resolves the text content and character index at a stage point for a text/field sprite.
    /// Returns (text, char_index) or None if the sprite has no text member.
    fn get_text_char_index_at_point(
        player: &DirPlayer,
        datum: &DatumRef,
        point_arg: &DatumRef,
    ) -> Result<Option<(String, usize)>, ScriptError> {
        let sprite_num = player.get_datum(datum).to_sprite_ref()?;
        let (vals, _flags) = player.get_datum(point_arg).to_point_inline()?;
        let stage_x = vals[0] as i32;
        let stage_y = vals[1] as i32;

        let sprite = match player.movie.score.get_sprite(sprite_num) {
            Some(s) => s,
            None => return Ok(None),
        };

        let member_ref = match &sprite.member {
            Some(r) => r.clone(),
            None => return Ok(None),
        };

        let sprite_rect = get_concrete_sprite_rect(player, sprite);
        let local_x = stage_x - sprite_rect.left;
        let local_y = stage_y - sprite_rect.top;

        let member = match player.movie.cast_manager.find_member_by_ref(&member_ref) {
            Some(m) => m,
            None => return Ok(None),
        };

        let (text, fixed_line_space, top_spacing) = match &member.member_type {
            CastMemberType::Text(t) => (t.text.clone(), t.fixed_line_space, t.top_spacing),
            CastMemberType::Field(f) => (f.text.clone(), f.fixed_line_space, f.top_spacing),
            _ => return Ok(None),
        };

        let font = player.font_manager.get_system_font().unwrap();
        let params = DrawTextParams {
            font: &font,
            line_height: None,
            line_spacing: fixed_line_space,
            top_spacing,
            char_spacing: 0,
            member_width: None,
            min_space_advance: None,
            per_char_advances: None,
        };

        let char_index = get_text_index_at_pos(&text, &params, local_x, local_y);
        Ok(Some((text, char_index)))
    }
}

/// The data needed to cross the Flash callback boundary. It intentionally
/// contains no VM borrows: setCallback prepares metadata and LocalConnection
/// bookkeeping while the selected owner is borrowed, releases that borrow for
/// host registration, then revalidates the captured owner/generation before
/// allocating the return value.
struct LingoCallbackRegistration {
    translated_path: String,
    flash_method: String,
    cast_lib: i32,
    cast_member: i32,
    lingo_name: String,
    flash_cast_lib: i32,
    flash_cast_member: i32,
    lc_target: Option<ScriptInstanceRef>,
}

impl SpriteDatumHandlers {
    fn prepare_lingo_callback(
        runtime: &mut ExecutionContext<'_>,
        datum: &DatumRef,
        args: &[DatumRef],
    ) -> Result<LingoCallbackRegistration, ScriptError> {
        if args.len() < 3 {
            return Err(ScriptError::new("setCallback requires a Flash object, method, and handler".to_owned()));
        }
        checked_datum(runtime, &args[0])?;
        if let Some(target) = args.get(3) {
            checked_datum(runtime, target)?;
        }
        let lingo_value = checked_datum(runtime, &args[2])?.clone();
        let lingo_handler = lingo_value
            .symbol_value(runtime.symbols)
            .unwrap_or_else(|_| Symbol::empty());
        let lingo_name = runtime
            .symbols
            .display(&lingo_handler)
            .unwrap_or_default()
            .to_owned();
        let flash_method = checked_datum(runtime, &args[1])
            .and_then(|value| value.string_value(runtime.symbols))?;
        runtime.with_player_and_symbols(|player, _symbols| {
            let receiver_sprite_num = player.get_datum(datum).to_sprite_ref()?;
            let flash_object_path = match player.get_datum(&args[0]) {
                Datum::FlashObjectRef(fo) => {
                    let (_, generation, sprite_num) = crate::player::handlers::datum_handlers::flash_object::validate_owned_binding(player, fo)?;
                    if sprite_num != receiver_sprite_num as i32 {
                        return Err(ScriptError::new_code(
                            crate::player::ScriptErrorCode::InvalidReference,
                            "Flash object targets a different sprite".to_owned(),
                        ));
                    }
                    if !player.is_flash_instance_generation_current(sprite_num as i16, generation) {
                        return Err(ScriptError::new_code(
                            crate::player::ScriptErrorCode::InvalidReference,
                            "Flash object targets a stale instance generation".to_owned(),
                        ));
                    }
                    fo.path.clone()
                }
                Datum::String(s) => s.clone(),
                other => {
                    return Err(ScriptError::new(format!(
                        "setCallback: first argument must be a Flash object or string, got {}",
                        other.type_str()
                    )));
                }
            };
            let translated_path = if flash_object_path.starts_with("_level0") {
                flash_object_path.replace("_level0", "_root")
            } else {
                flash_object_path
            };
            let (cast_lib, cast_member, lc_target) = if let Some(target) = args.get(3) {
                match player.get_datum(target) {
                    Datum::ScriptInstanceRef(script_ref) => {
                        let instance = player.allocator.get_script_instance(script_ref);
                        (instance.script.cast_lib, instance.script.cast_member, Some(script_ref.clone()))
                    }
                    _ => (0, 0, None),
                }
            } else {
                (0, 0, None)
            };
            let sprite_cast = player
                .movie
                .score
                .get_sprite(receiver_sprite_num)
                .and_then(|sprite| sprite.member.as_ref())
                .map(|member| (member.cast_lib, member.cast_member))
                .unwrap_or((0, 0));
            let (cast_lib, cast_member) = if cast_lib == 0 && cast_member == 0 {
                sprite_cast
            } else {
                (cast_lib, cast_member)
            };
            let (flash_cast_lib, flash_cast_member) = match player.get_datum(&args[0]) {
                Datum::FlashObjectRef(fo) => (fo.cast_lib, fo.cast_member),
                _ => sprite_cast,
            };
            Ok(LingoCallbackRegistration {
                translated_path,
                flash_method,
                cast_lib,
                cast_member,
                lingo_name,
                flash_cast_lib,
                flash_cast_member,
                lc_target,
            })
        })
    }

    fn register_lingo_callback(
        owner: &crate::player::ownership::OwnerToken,
        sprite_num: i16,
        generation: u64,
        registration: &LingoCallbackRegistration,
    ) -> Result<(), ScriptError> {
        JsApi::register_flash_lingo_callback(
            &owner_key_string(owner),
            sprite_num,
            generation,
            &registration.translated_path,
            &registration.flash_method,
            registration.cast_lib,
            registration.cast_member,
            &registration.lingo_name,
            registration.flash_cast_lib,
            registration.flash_cast_member,
        )
    }

    async fn execute_lingo_callback(
        session: crate::player::session::RuntimeSessionHandle,
        request: &crate::player::driver::SpriteAsyncRequest,
        sprite_num: i16,
        generation: u64,
        registration: LingoCallbackRegistration,
    ) -> Result<DatumRef, ScriptError> {
        crate::player::handlers::datum_handlers::flash_object::wait_for_flash_ready_owned(
            &request.owner,
            sprite_num as i32,
            generation,
        )
        .await?;
        // The session borrow is deliberately released before entering JS.
        Self::register_lingo_callback(&request.owner, sprite_num, generation, &registration)?;
        session
            .borrow_mut()
            .with_player(request.player_id, |context| {
                if !request.owner.same_identity(&context.player.owner)
                    || !request.owner.is_arena_live()
                    || context.player.flash_instance_generation(sprite_num) != Some(generation)
                {
                    return Err(crate::player::cancelled_scope_error());
                }
                Ok(context.player.alloc_datum(Datum::Int(1)))
            })
            .ok_or_else(crate::player::cancelled_scope_error)?
    }

    /// Execute Flash sprite variables through the owner-qualified request path.
    /// The request is completely prepared while the selected player is borrowed;
    /// host work and readiness waits happen after that borrow has been released.
    async fn execute_flash_variable_request(
        session: crate::player::session::RuntimeSessionHandle,
        request: &crate::player::driver::SpriteAsyncRequest,
        handler_name: &str,
    ) -> Result<DatumRef, ScriptError> {
        let is_get = handler_name.eq_ignore_ascii_case("getvariable");
        let prepared: (
            Option<crate::player::handlers::datum_handlers::flash_object::FlashRequest>,
            Vec<crate::player::FlashHostAction>,
            Option<DatumRef>,
        ) = session
            .borrow_mut()
            .with_player(request.player_id, |context| -> Result<_, ScriptError> {
                if !request.owner.same_identity(&context.player.owner)
                    || !request.owner.is_arena_live()
                {
                    return Err(crate::player::cancelled_scope_error());
                }
                let receiver_sprite = match checked_player_datum(
                    context.player,
                    context.symbols,
                    &request.receiver,
                )? {
                    Datum::SpriteRef(sprite_num) => *sprite_num,
                    _ => {
                        return Err(ScriptError::new_code(
                            ScriptErrorCode::InvalidReference,
                            "SpriteAsync receiver is not a sprite reference".to_owned(),
                        ));
                    }
                };
                if receiver_sprite != request.sprite_num {
                    return Err(ScriptError::new_code(
                        ScriptErrorCode::InvalidReference,
                        "SpriteAsync receiver sprite does not match its captured owner".to_owned(),
                    ));
                }

                let getter_inputs = if is_get {
                    let path = request
                        .args
                        .first()
                        .map(|arg| checked_player_datum(context.player, context.symbols, arg))
                        .transpose()?
                        .map(|datum| datum.string_value(context.symbols))
                        .transpose()?
                        .unwrap_or_default();
                    let return_as_object = request
                        .args
                        .get(1)
                        .map(|arg| {
                            checked_player_datum(context.player, context.symbols, arg)
                                .map(|datum| datum.int_value().unwrap_or(1) == 0)
                        })
                        .transpose()?
                        .unwrap_or(false);
                    Some((path, return_as_object))
                } else {
                    if request.args.len() < 2 {
                        return Err(ScriptError::new(
                            "setVariable requires a path and value".to_owned(),
                        ));
                    }
                    None
                };

                let Some((sprite_num, cast_lib, cast_member)) =
                    Self::resolve_sprite_flash_member_explicit(
                        context.player,
                        context.symbols,
                        &request.receiver,
                    )?
                else {
                    if let Some((path, true)) = getter_inputs.as_ref() {
                        let generation = context
                            .player
                            .flash_binding_state
                            .borrow_mut()
                            .reserve_absent(receiver_sprite)?;
                        let object = crate::director::lingo::datum::FlashObjectRef::from_path_with_sprite(
                            &root_flash_path(path),
                            0,
                            0,
                            receiver_sprite as i32,
                        )
                        .with_binding(
                            crate::player::owner_key_string(&context.player.owner),
                            generation,
                        );
                        return Ok((
                            None,
                            Vec::new(),
                            Some(context.player.alloc_datum(Datum::FlashObjectRef(object))),
                        ));
                    }
                    return Ok((None, Vec::new(), None));
                };

                let member_kind = context
                    .player
                    .movie
                    .cast_manager
                    .find_member_by_ref(&crate::player::cast_lib::CastMemberRef {
                        cast_lib,
                        cast_member,
                    })
                    .map(|member| {
                        matches!(&member.member_type, CastMemberType::Flash(flash)
                            if crate::rendering::has_swf_signature(&flash.data))
                    });
                if member_kind != Some(true) {
                    if is_get
                        && getter_inputs
                            .as_ref()
                            .is_some_and(|(_, return_as_object)| *return_as_object)
                        && member_kind.is_none()
                    {
                        let (path, _) = getter_inputs.as_ref().expect("getter inputs");
                        let generation = context
                            .player
                            .flash_binding_state
                            .borrow_mut()
                            .reserve_for_pair(receiver_sprite, cast_lib, cast_member)?;
                        let object = crate::director::lingo::datum::FlashObjectRef::from_path_with_sprite(
                            &root_flash_path(path),
                            cast_lib,
                            cast_member,
                            sprite_num as i32,
                        )
                        .with_binding(
                            crate::player::owner_key_string(&context.player.owner),
                            generation,
                        );
                        return Ok((
                            None,
                            Vec::new(),
                            Some(context.player.alloc_datum(Datum::FlashObjectRef(object))),
                        ));
                    }
                    if is_get
                        && getter_inputs
                            .as_ref()
                            .is_some_and(|(_, return_as_object)| *return_as_object)
                        && member_kind.is_some()
                    {
                        return Err(ScriptError::new_code(
                            ScriptErrorCode::InvalidReference,
                            "sprite getVariable object target is not a valid Flash member".to_owned(),
                        ));
                    }
                    return Ok((None, Vec::new(), None));
                }

                let (path, operation) = if is_get {
                    let (path, return_as_object) = getter_inputs
                        .as_ref()
                        .expect("getter inputs prepared before operation");
                    let path = path.clone();
                    let root_path = root_flash_path(&path);
                    let operation = crate::player::handlers::datum_handlers::flash_object::FlashOperation::SpriteGet {
                        return_mode: if *return_as_object {
                            crate::player::handlers::datum_handlers::flash_object::FlashReturnMode::Object {
                                fallback_path: Some(root_path.clone()),
                            }
                        } else {
                            crate::player::handlers::datum_handlers::flash_object::FlashReturnMode::Scalar
                        },
                    };
                    (root_path, operation)
                } else {
                    let path = checked_player_datum(
                        context.player,
                        context.symbols,
                        &request.args[0],
                    )?
                    .string_value(context.symbols)?;
                    let value = checked_player_datum(
                        context.player,
                        context.symbols,
                        &request.args[1],
                    )?
                    .string_value(context.symbols)?;
                    (
                        root_flash_path(&path),
                        crate::player::handlers::datum_handlers::flash_object::FlashOperation::Set {
                            value,
                        },
                    )
                };

                let already_loaded = context
                    .player
                    .flash_sprite_loaded
                    .contains(&(receiver_sprite, cast_lib, cast_member));
                if !already_loaded {
                    context.player.pre_dispatch_flash_members()?;
                }
                let generation = context
                    .player
                    .flash_instance_generation(receiver_sprite)
                    .ok_or_else(|| {
                        ScriptError::new_code(
                            ScriptErrorCode::InvalidReference,
                            "Flash sprite has no published instance generation".to_owned(),
                        )
                    })?;
                let current_pair = context
                    .player
                    .movie
                    .score
                    .get_sprite(receiver_sprite)
                    .and_then(|sprite| sprite.member.as_ref())
                    .map(|member| (member.cast_lib, member.cast_member));
                if current_pair != Some((cast_lib, cast_member)) {
                    return Err(ScriptError::new_code(
                        ScriptErrorCode::InvalidReference,
                        "Flash sprite variable cast binding was replaced during preparation".to_owned(),
                    ));
                }
                let request = crate::player::handlers::datum_handlers::flash_object::FlashRequest {
                    player_id: request.player_id,
                    owner: request.owner.clone(),
                    sprite_num: sprite_num as i32,
                    expected_generation: Some(generation),
                    path,
                    operation,
                    cast_lib,
                    cast_member,
                };
                Ok((
                    Some(request),
                    if already_loaded {
                        Vec::new()
                    } else {
                        context.player.take_flash_host_actions()
                    },
                    None,
                ))
            })
            .ok_or_else(crate::player::cancelled_scope_error)??;
        let (flash_request, actions, immediate) = prepared;
        if let Some(immediate) = immediate {
            return Ok(immediate);
        }

        let Some(flash_request) = flash_request else {
            return session
                .borrow_mut()
                .with_player(request.player_id, |context| {
                    Ok(context.player.alloc_datum(Datum::Void))
                })
                .ok_or_else(crate::player::cancelled_scope_error)?;
        };
        let actions = crate::player::bind_flash_host_actions(
            actions,
            session.clone(),
            request.player_id,
        );
        crate::player::emit_flash_host_actions(actions)?;
        let result = crate::player::commands::execute_owned_flash_request(
            &session,
            request.player_id,
            &request.owner,
            flash_request,
        )
        .await?;
        if is_get {
            return Ok(result);
        }
        session
            .borrow_mut()
            .with_player(request.player_id, |context| {
                Ok(context.player.alloc_datum(Datum::Void))
            })
            .ok_or_else(crate::player::cancelled_scope_error)?
    }

    pub(crate) async fn execute_async_request(
        session: crate::player::session::RuntimeSessionHandle,
        request: crate::player::driver::SpriteAsyncRequest,
    ) -> Result<DatumRef, ScriptError> {
        let owner_live = session
            .borrow_mut()
            .with_player(request.player_id, |context| {
                request.owner.same_identity(&context.player.owner)
                    && request.owner.is_arena_live()
            })
            .unwrap_or(false);
        if !owner_live {
            return Err(crate::player::cancelled_scope_error());
        }
        let (handler_name, flash_sprite) = session
            .borrow_mut()
            .with_player(request.player_id, |context| -> Result<(String, Option<i32>), ScriptError> {
                if !request.owner.same_identity(&context.player.owner)
                    || !request.owner.is_arena_live()
                {
                    return Err(crate::player::cancelled_scope_error());
                }
                let handler_name = context
                    .symbols
                    .display(&request.handler)
                    .map(str::to_owned)
                    .map_err(|_| ScriptError::new_code(
                        ScriptErrorCode::InvalidReference,
                        "foreign or stale sprite handler symbol".to_owned(),
                    ))?;
                let flash_sprite = Self::resolve_sprite_flash_member_explicit(
                    context.player,
                    context.symbols,
                    &request.receiver,
                )?
                .and_then(|(sprite_num, cast_lib, cast_member)| {
                    context
                        .player
                        .movie
                        .cast_manager
                        .find_member_by_ref(&crate::player::cast_lib::CastMemberRef {
                            cast_lib,
                            cast_member,
                        })
                        .filter(|member| {
                            matches!(
                                member.member_type,
                                CastMemberType::Flash(_)
                            )
                        })
                        .map(|_| sprite_num)
                });
                Ok((handler_name, flash_sprite))
            })
            .ok_or_else(crate::player::cancelled_scope_error)??;
        let handler_lower = handler_name.to_lowercase();
        if handler_lower == "setcallback" {
            if request.args.len() < 3 {
                return session
                    .borrow_mut()
                    .with_player(request.player_id, |context| {
                        Ok(context.player.alloc_datum(Datum::Void))
                    })
                    .ok_or_else(crate::player::cancelled_scope_error)?;
            }
            let Some(sprite_num) = flash_sprite.map(|number| number as i16) else {
                return Err(ScriptError::new("setCallback requires a Flash sprite".to_owned()));
            };
            // Prepare immutable callback metadata and reserve/capture the
            // exact binding generation in one validated owner borrow. The
            // captured generation cannot be adopted from a reentrant
            // replacement after host emission.
            let (registration, generation, actions) = session
                .borrow_mut()
                .with_player(request.player_id, |context| -> Result<_, ScriptError> {
                    if !request.owner.same_identity(&context.player.owner)
                        || !request.owner.is_arena_live()
                    {
                        return Err(crate::player::cancelled_scope_error());
                    }
                    let registration = Self::prepare_lingo_callback(
                        &mut ExecutionContext {
                            player_id: request.player_id,
                            player: context.player,
                            symbols: context.symbols,
                        },
                        &request.receiver,
                        &request.args,
                    )?;
                    let mut generation = context.player.flash_instance_generation(sprite_num);
                    let actions = if generation.is_none() {
                        context.player.pre_dispatch_flash_members()?;
                        generation = context.player.flash_instance_generation(sprite_num);
                        context.player.take_flash_host_actions()
                    } else {
                        Vec::new()
                    };
                    let generation = generation.ok_or_else(|| {
                        ScriptError::new("Flash callback binding has no current generation".to_owned())
                    })?;
                    if let Some(target_ref) = registration.lc_target.clone() {
                        context.player.flash_lc_callbacks.insert(
                            (registration.translated_path.clone(), registration.flash_method.clone()),
                            (registration.lingo_name.clone(), target_ref),
                        );
                    }
                    Ok((registration, generation, actions))
                })
                .ok_or_else(crate::player::cancelled_scope_error)??;
            let actions = crate::player::bind_flash_host_actions(
                actions,
                session.clone(),
                request.player_id,
            );
            if !actions.is_empty() {
                crate::player::emit_flash_host_actions(actions)?;
            }
            let still_current = session
                .borrow_mut()
                .with_player(request.player_id, |context| {
                    request.owner.same_identity(&context.player.owner)
                        && request.owner.is_arena_live()
                        && context.player.flash_instance_generation(sprite_num) == Some(generation)
                })
                .unwrap_or(false);
            if !still_current {
                return Err(crate::player::cancelled_scope_error());
            }
            return Self::execute_lingo_callback(
                session,
                &request,
                sprite_num,
                generation,
                registration,
            )
            .await;
        }
        if handler_lower == "callfunction" {
            let Some((flash_request, actions)) = session
                .borrow_mut()
                .with_player(request.player_id, |context| -> Result<_, ScriptError> {
                    if !request.owner.same_identity(&context.player.owner)
                        || !request.owner.is_arena_live()
                    {
                        return Err(crate::player::cancelled_scope_error());
                    }
                    let Some((sprite_num, cast_lib, cast_member)) =
                        Self::resolve_sprite_flash_member_explicit(
                            context.player,
                            context.symbols,
                            &request.receiver,
                        )?
                    else {
                        return Ok(None);
                    };
                    let is_flash = context
                        .player
                        .movie
                        .cast_manager
                        .find_member_by_ref(&crate::player::cast_lib::CastMemberRef {
                            cast_lib,
                            cast_member,
                        })
                        .is_some_and(|member| matches!(member.member_type, CastMemberType::Flash(_)));
                    if !is_flash {
                        return Ok(None);
                    }
                    let (path, args_xml) =
                        prepare_sprite_call_inputs(context.player, context.symbols, &request.args)?;
                    // Capture the current member pair and generation while the
                    // owner borrow is held. The owned executor revalidates both
                    // after readiness/host work before converting the result.
                    context.player.pre_dispatch_flash_members()?;
                    let current_pair = context
                        .player
                        .movie
                        .score
                        .get_sprite(sprite_num as i16)
                        .and_then(|sprite| sprite.member.as_ref())
                        .map(|member| (member.cast_lib, member.cast_member));
                    if current_pair != Some((cast_lib, cast_member)) {
                        return Err(ScriptError::new_code(
                            ScriptErrorCode::InvalidReference,
                            "Flash sprite call cast binding was replaced during preparation".to_owned(),
                        ));
                    }
                    let generation = context
                        .player
                        .flash_instance_generation(sprite_num as i16)
                        .ok_or_else(|| {
                            ScriptError::new_code(
                                ScriptErrorCode::InvalidReference,
                                "Flash sprite call has no current instance generation".to_owned(),
                            )
                        })?;
                    let flash_request =
                        crate::player::handlers::datum_handlers::flash_object::FlashObjectDatumHandlers::prepare_sprite_call(
                            context.player,
                            sprite_num as i16,
                            generation,
                            path,
                            args_xml,
                            cast_lib,
                            cast_member,
                        )?;
                    Ok(Some((flash_request, context.player.take_flash_host_actions())))
                })
                .ok_or_else(crate::player::cancelled_scope_error)??
            else {
                return session
                    .borrow_mut()
                    .with_player(request.player_id, |context| {
                        Ok(context.player.alloc_datum(Datum::Void))
                    })
                    .ok_or_else(crate::player::cancelled_scope_error)?;
            };
            let actions = crate::player::bind_flash_host_actions(
                actions,
                session.clone(),
                request.player_id,
            );
            if !actions.is_empty() {
                crate::player::emit_flash_host_actions(actions)?;
            }
            let result = crate::player::commands::execute_owned_flash_request(
                &session,
                request.player_id,
                &request.owner,
                flash_request,
            )
            .await?;
            return session
                .borrow_mut()
                .with_player(request.player_id, |context| {
                    if !request.owner.same_identity(&context.player.owner)
                        || !request.owner.is_arena_live()
                    {
                        return Err(crate::player::cancelled_scope_error());
                    }
                    if matches!(context.player.get_datum(&result), Datum::String(_)) {
                        Ok(result)
                    } else {
                        Ok(context.player.alloc_datum(Datum::Void))
                    }
                })
                .ok_or_else(crate::player::cancelled_scope_error)?;
        }
        if handler_lower == "getvariable" || handler_lower == "setvariable" {
            return Self::execute_flash_variable_request(
                session,
                &request,
                &handler_name,
            )
            .await;
        }
        if let Some(sprite_num) = flash_sprite {
            let json_args = session
                .borrow_mut()
                .with_player(request.player_id, |context| -> Result<String, ScriptError> {
                    let parts: Result<Vec<String>, ScriptError> = request
                        .args
                        .iter()
                        .map(|argument| {
                            let value = checked_player_datum(
                                context.player,
                                context.symbols,
                                argument,
                            )?;
                            Ok(match value {
                                Datum::Int(value) => value.to_string(),
                                Datum::Float(value) => value.to_string(),
                                Datum::String(value) => format!("{value:?}"),
                                Datum::Symbol(value) => format!(
                                    "{:?}",
                                    context.symbols.display(value).map_err(|_| {
                                        ScriptError::new_code(
                                            ScriptErrorCode::InvalidReference,
                                            "foreign or stale Flash argument symbol".to_owned(),
                                        )
                                    })?
                                ),
                                _ => "null".to_owned(),
                            })
                        })
                        .collect();
                    Ok(format!("[{}]", parts?.join(",")))
                })
                .ok_or_else(crate::player::cancelled_scope_error)??;
            match ruffle_call_function(
                sprite_num,
                &root_flash_path(&handler_name),
                &json_args,
            ) {
                Ok(value) => {
                    let datum = if let Some(value) = value.as_string() {
                        Datum::String(value)
                    } else if let Some(value) = value.as_bool() {
                        Datum::Int(if value { 1 } else { 0 })
                    } else if let Some(value) = value.as_f64() {
                        if value.fract() == 0.0 && value.abs() < i32::MAX as f64 {
                            Datum::Int(value as i32)
                        } else {
                            Datum::Float(value)
                        }
                    } else {
                        Datum::Void
                    };
                    return session
                        .borrow_mut()
                        .with_player(request.player_id, |context| {
                            if !request.owner.same_identity(&context.player.owner)
                                || !request.owner.is_arena_live()
                            {
                                return Err(crate::player::cancelled_scope_error());
                            }
                            Ok(context.player.alloc_datum(datum))
                        })
                        .ok_or_else(crate::player::cancelled_scope_error)?;
                }
                Err(error) => {
                    warn!("sprite direct Flash method '{}' error: {:?}", handler_name, error);
                }
            }
            return Ok(DatumRef::Void);
        }
        // Attached script handlers are started by RuntimeSession before this
        // direct host executor is reached. A non-Flash sprite with no such
        // handler follows Director's silent-void behavior.
        Ok(DatumRef::Void)
    }


    /// Resolve a sprite datum to (sprite_num, cast_lib, cast_member). The
    /// sprite_num is the lookup key for the per-sprite Ruffle instance;
    /// cast_lib/cast_member are still needed by callers that build
    /// `FlashObjectRef`s. Returns None when the sprite has no member.
    fn resolve_sprite_flash_member(datum: &DatumRef) -> Result<Option<(i32, i32, i32)>, ScriptError> {
        reserve_player_ref(|player| {
            let sprite_num = player.get_datum(datum).to_sprite_ref()?;
            let sprite = match player.movie.score.get_sprite(sprite_num) {
                Some(s) => s,
                None => return Ok(None),
            };
            match &sprite.member {
                Some(member_ref) => Ok(Some((
                    sprite_num as i32,
                    member_ref.cast_lib,
                    member_ref.cast_member,
                ))),
                None => Ok(None),
            }
        })
    }

    fn resolve_sprite_flash_member_explicit(
        player: &DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
    ) -> Result<Option<(i32, i32, i32)>, ScriptError> {
        let sprite_num = checked_player_datum(player, symbols, datum)?.to_sprite_ref()?;
        let Some(sprite) = player.movie.score.get_sprite(sprite_num) else {
            return Ok(None);
        };
        Ok(sprite.member.as_ref().map(|member| (
            sprite_num as i32, member.cast_lib, member.cast_member,
        )))
    }

    /// Returns true if the handler should be called via the async path.
    /// This returns true for:
    /// 1. Handlers found on the sprite's attached script instances
    /// 2. Any handler that isn't a built-in sync handler (to allow fallback to global handlers)
    pub fn has_async_handler(datum: &DatumRef, handler_name: &str) -> Result<bool, ScriptError> {
        // First check if it's a built-in sync handler (case-insensitive)
        let name_lower = handler_name.to_lowercase();

        // Flash interop: async ONLY on the first access to a sprite whose Ruffle
        // instance isn't confirmed ready yet — call_async waits for readiness,
        // then whitelists the sprite (flash_ready_sprites). Every later call
        // finds it whitelisted and takes the SYNC fast path below, so the
        // hundreds of per-frame interop calls (Coke Studios) don't pay
        // async-dispatch overhead. Cleared on member unload/swap so it re-waits.
        if matches!(
            name_lower.as_str(),
            "getvariable" | "setvariable" | "callfunction"
        ) {
            let ready = reserve_player_ref(|player| {
                match player.get_datum(datum).to_sprite_ref() {
                    Ok(sn) => player.flash_ready_sprites.contains(&sn),
                    Err(_) => false,
                }
            });
            return Ok(!ready);
        }
        if name_lower == "setcallback" {
            // Registration always crosses the owner-bound async boundary so
            // readiness, host acknowledgement, and the post-call generation
            // fence cannot be hidden behind a synchronous ambient lookup.
            return Ok(true);
        }

        let is_sync_handler = matches!(name_lower.as_str(),
            "intersects" | "getprop" | "getat" | "setat" | "getaprop" | "setaprop" | "pointtoword" | "pointtoline" |
            "gotoframe" | "callframe" | "stop" | "play" | "rewind" | "hold" |
            "newobject" |
            "hittest" | "getflashproperty" | "setflashproperty" | "telltarget" |
            "findlabel" | "flashtostage" | "stagetoflash" | "mapstagetomember" | "getpropref" |
            // `camera` belongs with the other 3D camera-list accessors: it must reach
            // SpriteDatumHandlers::call so `sprite(n).camera(i)` indexes the list.
            // Left off, it fell to the async path (sprite behaviours, then global
            // handlers) and the index was lost — every camera(i) answered with the
            // primary camera, so [PS] Fade's teardown never matched its own camera
            // and never removed it.
            "addcamera" | "removecamera" | "deletecamera" | "cameracount" | "camera"
        );
        if is_sync_handler {
            return Ok(false);
        }
        // pause and resume are not built-ins on an ordinary sprite, so they stay
        // on the async path where a behaviour may define them. On a sprite
        // showing an animated GIF they drive the animation.
        if matches!(name_lower.as_str(), "pause" | "resume")
            && crate::player::gif::sprite_has_gif(datum)
        {
            return Ok(false);
        }


        // For all other handlers, use the async path which will:
        // 1. Try sprite's attached scripts
        // 2. Fall back to global handlers
        Ok(true)
    }

    pub fn call(
        runtime: &mut ExecutionContext<'_>,
        datum: &DatumRef,
        handler_name: &str,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        let name_lower = handler_name.to_lowercase();
        match name_lower.as_str() {
            // A movie drives an animated GIF through its sprite:
            // `sprite(n).pause()` freezes it, `rewind()` returns it to frame 1
            // and `resume()` starts it again. Without these the animation runs
            // continuously, so a plume meant to puff at intervals never stops.
            "pause" | "resume" | "rewind" | "play" | "stop"
                if crate::player::gif::sprite_has_gif(datum) =>
            {
                crate::player::gif::control_sprite_gif(datum, &name_lower)
            }
            // `sprite(N).pointToChar(point)` — 1-based char index at a stage
            // coordinate within the sprite's text/field member, or -1 if the
            // point isn't within the text (Director 11.5 Scripting Dictionary).
            // Shares the hit-test core with `the mouseChar` / the global
            // `pointToChar()` builtin.
            "pointtochar" => {
                if args.is_empty() {
                    return Err(ScriptError::new(
                        "pointToChar requires 1 argument (point)".to_string(),
                    ));
                }
                let sprite_num = checked_sprite_num(runtime, datum)?;
                checked_datum(runtime, &args[0])?;
                runtime.with_player(|player| {
                    let (pt_vals, _f) = player.get_datum(&args[0]).to_point_inline()?;
                    let result = crate::player::compute_char_at(
                        player, sprite_num, pt_vals[0] as i32, pt_vals[1] as i32,
                    );
                    Ok(player.alloc_datum(Datum::Int(result)))
                })
            }
            "intersects" => {
                if args.is_empty() {
                    return Err(ScriptError::new(
                        "intersects requires 1 argument (sprite number)".to_string(),
                    ));
                }
                let sprite_num = checked_sprite_num(runtime, datum)?;
                checked_datum(runtime, &args[0])?;
                runtime.with_player(|player| {
                    let other_sprite_num =
                        player.get_datum(&args[0]).int_value()? as i16;

                    // Get both sprites' rects
                    let sprite1 = player.movie.score.get_sprite(sprite_num);
                    let sprite2 = player.movie.score.get_sprite(other_sprite_num);

                    if sprite1.is_none() || sprite2.is_none() {
                        return Ok(player.alloc_datum(Datum::Int(0)));
                    }

                    let sprite1 = sprite1.unwrap();
                    let sprite2 = sprite2.unwrap();

                    // Get the concrete rects of both sprites
                    let rect1 = get_concrete_sprite_rect(player, sprite1);
                    let rect2 = get_concrete_sprite_rect(player, sprite2);

                    // Check if rectangles intersect
                    let intersects = !(
                        rect1.right <= rect2.left
                            || rect1.left >= rect2.right
                            || rect1.bottom <= rect2.top
                            || rect1.top >= rect2.bottom
                    );

                    Ok(player.alloc_datum(Datum::Int(if intersects { 1 } else { 0 })))
                })
            }
            // `sprite(n).camera(i)` — INDEXED access into the sprite's camera list.
            // Without it the index was ignored and every `camera(i)` answered with the
            // primary camera, so [PS] Fade's teardown
            //     repeat with i = 1 to tCount
            //       if tSprite.camera(i).name = p.camera.name then tIndex = i
            //     if tIndex > 0 then tSprite.deleteCamera(tIndex)
            // never found its own camera and never removed it. Every fade then left
            // another Fade_Camera pass on the sprite; they stacked up, and one whose
            // clearAtRender defaulted to TRUE wiped the skybox, the 3D world and the
            // menu UI that had already been drawn that frame.
            "camera" => {
                let sprite_num = checked_sprite_num(runtime, datum)?;
                if let Some(arg) = args.first() { checked_datum(runtime, arg)?; }
                let player = &mut *runtime.player;
                {
                    let index = if !args.is_empty() {
                        player.get_datum(&args[0]).int_value().unwrap_or(1)
                    } else { 1 };
                    let own = player.movie.score.get_sprite(sprite_num)
                        .and_then(|s| s.member.as_ref())
                        .cloned()
                        .unwrap_or(crate::player::cast_lib::NULL_CAST_MEMBER_REF);
                    let cams = sprite_camera_list(player, &*runtime.symbols, sprite_num as i16)?;
                    let idx = (index.max(1) as usize) - 1;
                    match cams.get(idx) {
                        Some(c) => {
                            let (cast_lib, cast_member) =
                                c.member.unwrap_or((own.cast_lib, own.cast_member));
                            Ok(player.alloc_datum(Datum::Shockwave3dObjectRef(
                                crate::director::lingo::datum::Shockwave3dObjectRef {
                                    cast_lib,
                                    cast_member,
                                    object_type: BuiltInSymbol::Camera,
                                    name: runtime.symbols.intern(&c.name),
                                },
                            )))
                        }
                        None => Ok(DatumRef::Void),
                    }
                }
            }
            "cameracount" => {
                let sprite_num = checked_sprite_num(runtime, datum)?;
                let player = &mut *runtime.player;
                {
                    let count = sprite_camera_list(player, &*runtime.symbols, sprite_num as i16)?.len().max(1) as i32;
                    Ok(player.alloc_datum(Datum::Int(count)))
                }
            }
            "addcamera" => {
                let sprite_num = checked_sprite_num(runtime, datum)?;
                if let Some(arg) = args.first() { checked_datum(runtime, arg)?; }
                if let Some(arg) = args.get(1) { checked_datum(runtime, arg)?; }
                let player = &mut *runtime.player;
                {
                    // addCamera(cameraRef, index)
                    // index 1 = primary camera, 2+ = additional cameras rendered on top
                    // A camera reference carries the member that owns it; that member
                    // may differ from the sprite's own (Director composites each camera
                    // against its own member's world).
                    let cam_entry = if !args.is_empty() {
                        match player.get_datum(&args[0]) {
                            Datum::Shockwave3dObjectRef(r) => Some(crate::player::sprite::SpriteCamera {
                                member: Some((r.cast_lib, r.cast_member)),
                                name: runtime
                                    .symbols
                                    .display(&r.name)
                                    .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
                                    .to_owned(),
                            }),
                            Datum::String(s) if !s.is_empty() => Some(crate::player::sprite::SpriteCamera {
                                member: None,
                                name: s.clone(),
                            }),
                            _ => None,
                        }
                    } else { None };
                    let index = if args.len() >= 2 {
                        Some(player.get_datum(&args[1]).int_value().unwrap_or(1) as usize)
                    } else { None };
                    if let Some(cam_entry) = cam_entry {
                        // Director 11.5 Scripting Dictionary, `addCamera`: "adds a camera
                        // to the list of cameras for the sprite … index … specifies the
                        // index in the list of cameras at which whichCamera is ADDED. If
                        // index is greater than the value of cameraCount(), the camera is
                        // added to the end of the list." Its example "inserts the camera
                        // named FlightCam at the fifth index position" — so an existing
                        // camera at that index shifts down, it is NOT replaced.
                        //
                        // Index 1 used to overwrite the primary camera outright, which
                        // silently DISCARDED it. AreaZero's [PS] Camera does
                        //     p.sprite.camera = Player1_Camera
                        //     p.sprite.addCamera(tSkyboxCamera, 1)
                        // so the camera the whole menu is framed through was thrown away
                        // the moment the skybox camera was added, and the sprite rendered
                        // through an unrelated fallback view that never moved.
                        //
                        // The sprite stores the list as primary + extras; treat them as
                        // one ordered list so an insert anywhere shifts correctly. The
                        // list already holds the member's default camera at index 1 (see
                        // sprite_camera_list), so a plain append layers over the world
                        // rather than replacing it.
                        let mut cams = sprite_camera_list(player, &*runtime.symbols, sprite_num as i16)?;
                        let at = match index {
                            Some(i) => i.saturating_sub(1).min(cams.len()),
                            None => cams.len(), // addCamera(cam) — append
                        };
                        cams.insert(at, cam_entry);
                        set_sprite_camera_list(player, sprite_num as i16, cams);
                    }
                    Ok(player.alloc_datum(Datum::Void))
                }
            }
            // Director 11.5 Scripting Dictionary, `deleteCamera`:
            // "sprite(whichSprite).deleteCamera(cameraOrIndex) … removes the camera
            // from the sprite's list of cameras. The camera is not deleted from the
            // cast member." The argument is "a string or an integer that specifies the
            // name or index position", and the doc's own example also passes a camera
            // REFERENCE — so accept all three. Only `removeCamera` existed before, so
            // the documented `deleteCamera` fell through to the async path and did
            // nothing: [PS] Fade could never drop its camera.
            "deletecamera" | "removecamera" => {
                let sprite_num = checked_sprite_num(runtime, datum)?;
                if let Some(arg) = args.first() { checked_datum(runtime, arg)?; }
                let player = &mut *runtime.player;
                {
                    let target = args.first().map(|a| player.get_datum(a).clone());
                    // Mirror addCamera: one ordered list, so removing index 1 promotes
                    // the next camera instead of being a silent no-op.
                    let mut cams = sprite_camera_list(player, &*runtime.symbols, sprite_num as i16)?;
                    let at = match &target {
                        Some(Datum::Shockwave3dObjectRef(r)) => {
                            let target_name = runtime
                                .symbols
                                .lower(&r.name)
                                .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
                            cams.iter().position(|c| target_name.eq_ignore_ascii_case(&c.name))
                        }
                        Some(Datum::String(name)) => cams
                            .iter()
                            .position(|c| c.name.eq_ignore_ascii_case(name)),
                        Some(other) => other
                            .int_value()
                            .ok()
                            .map(|i| (i.max(1) as usize) - 1),
                        None => None,
                    };
                    if let Some(at) = at {
                        if at < cams.len() {
                            cams.remove(at);
                        }
                    }
                    set_sprite_camera_list(player, sprite_num as i16, cams);
                    Ok(player.alloc_datum(Datum::Void))
                }
            }
            "getprop" => {
                if args.is_empty() {
                    return Err(ScriptError::new(
                        "getProp requires at least 1 argument".to_string(),
                    ));
                }
                let sprite_num = checked_sprite_num(runtime, datum)?;
                checked_datum(runtime, &args[0])?;
                let player = &mut *runtime.player;
                let symbols = &mut *runtime.symbols;
                {
                    // Get the property name from the first arg
                    let prop_name = player.get_datum(&args[0]).symbol_value(symbols)?;

                    // First, try to get it as a built-in sprite property
                    match crate::player::score::sprite_get_prop(
                        player,
                        symbols,
                        sprite_num as i16,
                        prop_name.clone(),
                    ) {
                        Ok(prop_datum) => {
                            let result = player.last_sprite_prop_ref.take()
                                .unwrap_or_else(|| player.alloc_datum(prop_datum));
                            checked_player_datum(player, symbols, &result)?;

                            // If there's a second argument, it's a sub-property access
                            if args.len() > 1 {
                                checked_player_datum(player, symbols, &args[1])?;
                                let sub = crate::player::handlers::types::TypeUtils::get_sub_prop(
                                    &result, &args[1], player, symbols,
                                )?;
                                checked_player_datum(player, symbols, &sub)?;
                                return Ok(sub);
                            }

                            return Ok(result);
                        }
                        Err(error) if error.code == ScriptErrorCode::InvalidReference => return Err(error),
                        Err(_) => {
                            // Not a built-in sprite property, try script instances
                        }
                    }

                    let sprite = player.movie.score.get_sprite(sprite_num);
                    if sprite.is_none() {
                        return Err(ScriptError::new(format!("Sprite {} not found", sprite_num)));
                    }

                    // Clone the script instance list to avoid borrow conflicts
                    let instance_refs = sprite.unwrap().script_instance_list.clone();

                    // Try to get the property from the sprite's script instances
                    for instance_ref in instance_refs {
                        checked_script_instance(player, &instance_ref)?;
                        match script_get_prop(
                            player,
                            symbols,
                            &instance_ref,
                            prop_name.clone(),
                        ) {
                            Ok(result) => {
                                checked_player_datum(player, symbols, &result)?;
                                // If there's a second argument, it's a sub-property access
                                if args.len() > 1 {
                                    checked_player_datum(player, symbols, &args[1])?;
                                    let sub = crate::player::handlers::types::TypeUtils::get_sub_prop(
                                        &result, &args[1], player, symbols,
                                    )?;
                                    checked_player_datum(player, symbols, &sub)?;
                                    return Ok(sub);
                                }
                                return Ok(result);
                            }
                            Err(error) if error.code == ScriptErrorCode::InvalidReference => return Err(error),
                            Err(_) => {}
                        }
                    }

                    // If not found anywhere, return void
                    Ok(DatumRef::Void)
                }
            }
            // getAt / getaProp: bracket access on sprite, e.g. sprite(9)[#pLevel]
            "getat" | "getaprop" => {
                if args.is_empty() {
                    return Err(ScriptError::new(
                        "getAt requires 1 argument".to_string(),
                    ));
                }
                let sprite_num = checked_sprite_num(runtime, datum)?;
                checked_datum(runtime, &args[0])?;
                let player = &mut *runtime.player;
                let symbols = &mut *runtime.symbols;
                {
                    let prop_name = player.get_datum(&args[0]).symbol_value(symbols)?;

                    // Try built-in sprite property first
                    match crate::player::score::sprite_get_prop(
                        player,
                        symbols,
                        sprite_num as i16,
                        prop_name.clone(),
                    ) {
                        Ok(prop_datum) => {
                            let result = player.last_sprite_prop_ref.take()
                                .unwrap_or_else(|| player.alloc_datum(prop_datum));
                            checked_player_datum(player, symbols, &result)?;
                            return Ok(result);
                        }
                        Err(error) if error.code == ScriptErrorCode::InvalidReference => return Err(error),
                        Err(_) => {}
                    }

                    // Fall back to sprite's script instance properties. Same
                    // rule as `sprite_get_prop`: resolve through
                    // get_sprite_script_instance_ids so behaviours attached at
                    // runtime via `scriptInstanceList.add/addAt` are included —
                    // the sprite's internal Vec holds only score-authored ones.
                    let sprite = player.movie.score.get_sprite(sprite_num);
                    if sprite.is_none() {
                        return Ok(DatumRef::Void);
                    }
                    let fallback = sprite.unwrap().script_instance_list.clone();
                    let instance_refs = crate::player::score::get_sprite_script_instance_ids_checked(
                        player,
                        &*symbols,
                        sprite_num,
                        fallback.as_slice(),
                    )?;
                    for instance_ref in instance_refs {
                        checked_script_instance(player, &instance_ref)?;
                        match script_get_prop(player, symbols, &instance_ref, prop_name.clone()) {
                            Ok(result) => {
                                checked_player_datum(player, symbols, &result)?;
                                return Ok(result);
                            }
                            Err(error) if error.code == ScriptErrorCode::InvalidReference => return Err(error),
                            Err(_) => {}
                        }
                    }

                    Ok(DatumRef::Void)
                }
            }
            "getpropref" => {
                if args.is_empty() {
                    return Err(ScriptError::new(
                        "getPropRef requires at least 1 argument".to_string(),
                    ));
                }
                let sprite_num = checked_sprite_num(runtime, datum)?;
                checked_datum(runtime, &args[0])?;
                let player = &mut *runtime.player;
                let symbols = &mut *runtime.symbols;
                {
                    let prop_name = player.get_datum(&args[0]).symbol_value(symbols)?;

                    // Get the property value (this handles scriptInstanceList cache etc.)
                    match crate::player::score::sprite_get_prop(
                        player,
                        symbols,
                        sprite_num as i16,
                        prop_name.clone(),
                    ) {
                        Ok(prop_datum) => {
                            let result = player.last_sprite_prop_ref.take()
                                .unwrap_or_else(|| player.alloc_datum(prop_datum));
                            checked_player_datum(player, symbols, &result)?;

                            // If there's a second argument, it's an index into
                            // the property value. A property list is indexed by
                            // KEY (symbol/string), not a positional int — e.g.
                            // Summer Resort's `sprite(i).pData[#item]`, where
                            // pData is a #-keyed prop list. The old code coerced
                            // the key via int_value() (a symbol → 0) and only
                            // handled positional Lists, so it errored "cannot
                            // index into prop_list with 0". Mirror the PropList
                            // handler's key lookup here.
                            if args.len() > 1 {
                                checked_player_datum(player, symbols, &args[1])?;
                                let list_datum = player.get_datum(&result).clone();
                                match list_datum {
                                    Datum::PropList(pairs, pairs_sorted) => {
                                        let selected = crate::player::handlers::datum_handlers::prop_list::PropListUtils::get_by_key(
                                            &pairs, &args[1], &player.allocator, symbols, pairs_sorted,
                                        )?;
                                        checked_player_datum(player, symbols, &selected)?;
                                        return Ok(selected);
                                    }
                                    Datum::List(_, item_refs, _) => {
                                        let index = player.get_datum(&args[1]).int_value()?;
                                        if index < 1 || index as usize > item_refs.len() {
                                            return Err(ScriptError::new(format!(
                                                "getPropRef: index {} out of range for list of length {}",
                                                index, item_refs.len()
                                            )));
                                        }
                                        let selected = item_refs[(index - 1) as usize].clone();
                                        checked_player_datum(player, symbols, &selected)?;
                                        return Ok(selected);
                                    }
                                    _ => {
                                        let index = player.get_datum(&args[1]).int_value().unwrap_or(0);
                                        return Err(ScriptError::new(format!(
                                            "getPropRef: cannot index into {} with {}",
                                            player.get_datum(&result).type_str(), index
                                        )));
                                    }
                                }
                            }

                            return Ok(result);
                        }
                        Err(error) if error.code == ScriptErrorCode::InvalidReference => return Err(error),
                        Err(_) => {
                            // Not a built-in sprite property, try script instances
                        }
                    }

                    // Fall back to script instance properties
                    let sprite = player.movie.score.get_sprite(sprite_num);
                    if sprite.is_none() {
                        return Err(ScriptError::new(format!("Sprite {} not found", sprite_num)));
                    }
                    let instance_refs = sprite.unwrap().script_instance_list.clone();
                    for instance_ref in instance_refs {
                        checked_script_instance(player, &instance_ref)?;
                        match script_get_prop(player, symbols, &instance_ref, prop_name.clone()) {
                            Ok(result) => {
                                checked_player_datum(player, symbols, &result)?;
                                if args.len() > 1 {
                                    checked_player_datum(player, symbols, &args[1])?;
                                    let sub = crate::player::handlers::types::TypeUtils::get_sub_prop(
                                        &result, &args[1], player, symbols,
                                    )?;
                                    checked_player_datum(player, symbols, &sub)?;
                                    return Ok(sub);
                                }
                                return Ok(result);
                            }
                            Err(error) if error.code == ScriptErrorCode::InvalidReference => return Err(error),
                            Err(_) => {}
                        }
                    }

                    Ok(DatumRef::Void)
                }
            }
            // setAt / setaProp: bracket assignment on sprite, e.g. sprite(9)[#pLevel] = value
            "setat" | "setaprop" => {
                if args.len() < 2 {
                    return Err(ScriptError::new(
                        "setAt requires 2 arguments".to_string(),
                    ));
                }
                let sprite_num = checked_sprite_num(runtime, datum)?;
                checked_datum(runtime, &args[0])?;
                let prop_name = {
                    let player = &mut *runtime.player;
                    let symbols = &mut *runtime.symbols;
                    player.get_datum(&args[0]).symbol_value(symbols)?
                };
                checked_datum(runtime, &args[1])?;
                let player = &mut *runtime.player;
                let symbols = &mut *runtime.symbols;
                {
                    let value = player.get_datum(&args[1]).clone();
                    let value_ref = &args[1];

                    // Try built-in sprite property first
                    match crate::player::score::sprite_set_prop(
                        player,
                        symbols,
                        sprite_num as i16,
                        prop_name.clone(),
                        value,
                    ) {
                        Ok(_) => return Ok(DatumRef::Void),
                        Err(error) if error.code == ScriptErrorCode::InvalidReference => return Err(error),
                        Err(_) => {}
                    }

                    // Fall back to sprite's script instance properties
                    let sprite = player.movie.score.get_sprite(sprite_num);
                    if sprite.is_none() {
                        return Err(ScriptError::new(format!("Sprite {} not found", sprite_num)));
                    }
                    let instance_refs = sprite.unwrap().script_instance_list.clone();
                    for instance_ref in instance_refs {
                        checked_script_instance(player, &instance_ref)?;
                        match script_set_prop(player, symbols, &instance_ref, prop_name.clone(), value_ref, false) {
                            Ok(()) => return Ok(DatumRef::Void),
                            Err(error) if error.code == ScriptErrorCode::InvalidReference => return Err(error),
                            Err(_) => {}
                        }
                    }

                    let prop_display = symbols
                        .display(&prop_name)
                        .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
                    Err(ScriptError::new(format!(
                        "Property {} not found on sprite {}", prop_display, sprite_num
                    )))
                }
            }
            "pointtoword" => {
                if args.is_empty() {
                    return Err(ScriptError::new(
                        "pointToWord requires 1 argument (point)".to_string(),
                    ));
                }
                let _sprite_num = checked_sprite_num(runtime, datum)?;
                checked_datum(runtime, &args[0])?;
                runtime.with_player(|player| {
                    let (text, char_index) = match SpriteDatumUtils::get_text_char_index_at_point(player, datum, &args[0])? {
                        Some(r) => r,
                        None => return Ok(player.alloc_datum(Datum::Int(-1))),
                    };

                    // Find which word (1-based) the character at char_index belongs to
                    let mut word_num = 0;
                    let mut char_count = 0;
                    let mut in_word = false;
                    for c in text.chars() {
                        if c.is_whitespace() {
                            in_word = false;
                        } else if !in_word {
                            word_num += 1;
                            in_word = true;
                        }
                        if char_count == char_index {
                            return Ok(player.alloc_datum(Datum::Int(word_num)));
                        }
                        char_count += 1;
                    }

                    // Past end of text: return the last word number
                    Ok(player.alloc_datum(Datum::Int(word_num)))
                })
            }
            "pointtoline" => {
                if args.is_empty() {
                    return Err(ScriptError::new(
                        "pointToLine requires 1 argument (point)".to_string(),
                    ));
                }
                let _sprite_num = checked_sprite_num(runtime, datum)?;
                checked_datum(runtime, &args[0])?;
                runtime.with_player(|player| {
                    let (text, char_index) = match SpriteDatumUtils::get_text_char_index_at_point(player, datum, &args[0])? {
                        Some(r) => r,
                        None => return Ok(player.alloc_datum(Datum::Int(-1))),
                    };

                    // Find which line (1-based) the character at char_index belongs to
                    let mut line_num = 1;
                    let mut char_count = 0;
                    for c in text.chars() {
                        if char_count == char_index {
                            return Ok(player.alloc_datum(Datum::Int(line_num)));
                        }
                        if c == '\r' || c == '\n' {
                            line_num += 1;
                        }
                        char_count += 1;
                    }

                    // Past end of text: return the last line number
                    Ok(player.alloc_datum(Datum::Int(line_num)))
                })
            }
            // Flash (SWF) sprite methods
            "gotoframe" => {
                if let Some((sn, _cl, _cm)) = Self::resolve_sprite_flash_member_explicit(runtime.player, runtime.symbols, datum)? {
                    // Pass the raw arg as a string — Lingo callers use
                    // either a numeric frame or a string label (e.g.
                    // `sprite(N).gotoFrame("warm0")`). The JS bridge
                    // parses the string and routes to GotoFrame(int) or
                    // an AS1 `gotoAndStop(label)` call as appropriate.
                    let frame_or_label = runtime.with_player_and_symbols(|player, symbols| {
                        if args.is_empty() { return Ok("1".to_string()); }
                        player.get_datum(&args[0]).string_value(symbols)
                    })?;
                    ruffle_goto_frame(sn, &frame_or_label);
                }
                Ok(DatumRef::Void)
            }
            "callframe" => {
                if let Some((sn, _cl, _cm)) = Self::resolve_sprite_flash_member_explicit(runtime.player, runtime.symbols, datum)? {
                    let frame = runtime.with_player_and_symbols(|player, symbols| {
                        if args.is_empty() { return Ok(1); }
                        player.get_datum(&args[0]).int_value()
                    })?;
                    ruffle_call_frame(sn, frame);
                }
                Ok(DatumRef::Void)
            }
            "stop" => {
                if let Some((sn, _cl, _cm)) = Self::resolve_sprite_flash_member_explicit(runtime.player, runtime.symbols, datum)? {
                    ruffle_stop(sn);
                }
                Ok(DatumRef::Void)
            }
            "play" => {
                if let Some((sn, _cl, _cm)) = Self::resolve_sprite_flash_member_explicit(runtime.player, runtime.symbols, datum)? {
                    // play() overrides a prior `sprite.frame = N` hold — clear
                    // the asserted frame so a fresh instance plays, not pins.
                    runtime.with_player_and_symbols(|player, symbols| {
                        player.movie.score.get_sprite_mut(sn as i16).flash_asserted_frame = None;
                    });
                    let owner_key = owner_key_string(&runtime.player.owner);
                    ruffle_play_owned(&owner_key, sn);
                }
                Ok(DatumRef::Void)
            }
            "rewind" => {
                if let Some((sn, _cl, _cm)) = Self::resolve_sprite_flash_member_explicit(runtime.player, runtime.symbols, datum)? {
                    ruffle_rewind(sn);
                }
                Ok(DatumRef::Void)
            }
            // Director 11.5 `hold()` (spec: "stops a Flash movie sprite that is
            // playing in the current frame, but any audio continues to play").
            // We map it to the same root-timeline stop as `stop()` — stopFlash
            // halts the root MovieClip while leaving the player alive to render
            // (we don't split audio in the Ruffle bridge, an acceptable
            // divergence). bogey_nights' bogeyman #hiding relies on hold to
            // actually halt the idle SWF.
            "hold" => {
                if let Some((sn, _cl, _cm)) = Self::resolve_sprite_flash_member_explicit(runtime.player, runtime.symbols, datum)? {
                    ruffle_stop(sn);
                }
                Ok(DatumRef::Void)
            }
            "getvariable" => {
                Err(ScriptError::new(
                    "sprite.getVariable requires the owner-bound async path".to_owned(),
                ))
            }
            "setvariable" => {
                Err(ScriptError::new(
                    "sprite.setVariable requires the owner-bound async path".to_owned(),
                ))
            }
            "callfunction" => {
                if let Some((sn, _cl, _cm)) = Self::resolve_sprite_flash_member_explicit(runtime.player, runtime.symbols, datum)? {
                    let path = runtime.with_player_and_symbols(|player, symbols| {
                        player.get_datum(&args[0]).string_value(symbols).map(|p| root_flash_path(&p))
                    })?;
                    let args_xml = if args.len() > 1 {
                        runtime.with_player_and_symbols(|player, symbols| {
                            player.get_datum(&args[1]).string_value(symbols)
                        })?
                    } else {
                        String::new()
                    };
                    match ruffle_call_function(sn, &path, &args_xml) {
                        Ok(val) => {
                            if let Some(s) = val.as_string() {
                                return runtime.with_player_and_symbols(|player, symbols| {
                                    Ok(player.alloc_datum(Datum::String(s)))
                                });
                            }
                        }
                        Err(e) => warn!("sprite.callFunction error: {:?}", e),
                    }
                }
                Ok(DatumRef::Void)
            }
            "hittest" => {
                if let Some((sn, _cl, _cm)) = Self::resolve_sprite_flash_member_explicit(runtime.player, runtime.symbols, datum)? {
                    // Director's `sprite.hitTest(point)` takes a *stage* point
                    // (e.g. `sprite(5).hitTest(_mouse.mouseLoc)`). Accept a
                    // Point datum, or a legacy two-int (x, y) form. Rebase to
                    // sprite-local pixels (the classifier's coordinate space)
                    // by subtracting the sprite's top-left.
                    let (sx, sy) = runtime.with_player_and_symbols(|player, symbols| -> Result<(i32, i32), ScriptError> {
                        match player.get_datum(&args[0]) {
                            Datum::Point([px, py], _) => Ok((*px as i32, *py as i32)),
                            other => {
                                let x = other.int_value()?;
                                let y = if let Some(a) = args.get(1) {
                                    player.get_datum(a).int_value()?
                                } else {
                                    0
                                };
                                Ok((x, y))
                            }
                        }
                    })?;
                    let rect = runtime.with_player_and_symbols(|player, symbols| {
                        get_sprite_rect_in_context(player, sn as i16)
                    });
                    let lx = (sx - rect.0 as i32) as f64;
                    let ly = (sy - rect.1 as i32) as f64;
                    // 0 = #background, 1 = #normal, 2 = #button, 3 = #editText.
                    let symbol = match ruffle_hit_test(sn, lx, ly) {
                        2 => "button",
                        3 => "editText",
                        1 => "normal",
                        _ => "background",
                    };
                    let symbol = runtime.symbols.intern(symbol);
                    return Ok(runtime.player.alloc_datum(Datum::Symbol(symbol)));
                }
                Ok(DatumRef::Void)
            }
            "getflashproperty" => {
                if let Some((sn, _cl, _cm)) = Self::resolve_sprite_flash_member_explicit(runtime.player, runtime.symbols, datum)? {
                    let target = runtime.with_player_and_symbols(|player, symbols| player.get_datum(&args[0]).string_value(symbols))?;
                    let prop_num = runtime.with_player_and_symbols(|player, symbols| player.get_datum(&args[1]).int_value())?;
                    match ruffle_get_flash_property(sn, &target, prop_num) {
                        Ok(val) => {
                            if let Some(s) = val.as_string() {
                                return runtime.with_player_and_symbols(|player, symbols| {
                                    Ok(player.alloc_datum(Datum::String(s)))
                                });
                            }
                        }
                        Err(e) => warn!("sprite.getFlashProperty error: {:?}", e),
                    }
                }
                Ok(DatumRef::Void)
            }
            "setflashproperty" => {
                if let Some((sn, _cl, _cm)) = Self::resolve_sprite_flash_member_explicit(runtime.player, runtime.symbols, datum)? {
                    let target = runtime.with_player_and_symbols(|player, symbols| player.get_datum(&args[0]).string_value(symbols))?;
                    let prop_num = runtime.with_player_and_symbols(|player, symbols| player.get_datum(&args[1]).int_value())?;
                    let value = runtime.with_player_and_symbols(|player, symbols| player.get_datum(&args[2]).string_value(symbols))?;
                    ruffle_set_flash_property(sn, &target, prop_num, &value);
                }
                Ok(DatumRef::Void)
            }
            "setcallback" => {
                // setCallback(flashObject, flashMethod, lingoHandler, lingoTarget)
                if args.len() >= 3 {
                    return Err(ScriptError::new(
                        "setCallback requires the owner-bound async path".to_owned(),
                    ));
                } else {
                    Ok(DatumRef::Void)
                }
            }
            "mapstagetomember" => {
                runtime.with_player_and_symbols(|player, symbols| {
                    if args.is_empty() {
                        return Err(ScriptError::new(
                            "mapStageToMember requires 1 argument (point)".to_string(),
                        ));
                    }
                    let sprite_num = player.get_datum(datum).to_sprite_ref()?;
                    let (vals, _flags) = player.get_datum(&args[0]).to_point_inline().map_err(|_| ScriptError::new(
                        "mapStageToMember requires a point argument".to_string(),
                    ))?;
                    let (stage_x, stage_y) = (vals[0] as i32, vals[1] as i32);

                    let sprite = player.movie.score.get_sprite(sprite_num);
                    if sprite.is_none() {
                        return Ok(DatumRef::Void);
                    }
                    let sprite = sprite.unwrap();

                    let rect = get_concrete_sprite_rect(player, sprite);

                    // Convert stage coords to member-local coords
                    let member = sprite.member.as_ref()
                        .and_then(|mr| player.movie.cast_manager.find_member_by_ref(mr));

                    let (member_w, member_h) = if let Some(m) = &member {
                        match &m.member_type {
                            CastMemberType::Bitmap(bm) => (bm.info.width as i32, bm.info.height as i32),
                            _ => (rect.right - rect.left, rect.bottom - rect.top),
                        }
                    } else {
                        (rect.right - rect.left, rect.bottom - rect.top)
                    };

                    let sprite_w = rect.right - rect.left;
                    let sprite_h = rect.bottom - rect.top;

                    if sprite_w == 0 || sprite_h == 0 {
                        return Ok(DatumRef::Void);
                    }

                    // Map stage point to member coordinates, accounting for scaling
                    let local_x = (stage_x - rect.left) * member_w / sprite_w;
                    let local_y = (stage_y - rect.top) * member_h / sprite_h;

                    Ok(player.alloc_datum(Datum::Point([local_x as f64, local_y as f64], 0)))
                })
            }
            // `sprite(N).findLabel(whichLabelName)` — "returns the frame
            // number (within the Flash movie) that is associated with the
            // label name requested. A 0 is returned if the label doesn't
            // exist, or if that portion of the Flash movie has not yet been
            // streamed in" (Director 11.5 Scripting Dictionary, findLabel()).
            //
            // Resolved from the member's SWF bytes, not from Ruffle: frame
            // labels are static SWF content, so this answers correctly even
            // before the Ruffle instance has loaded (and the legacy Flash
            // Player JS API exposes no label lookup to ask in the first
            // place). Movies drive whole animation state machines off this —
            // monsterattack stores `#start: findLabel("AttackT"), #end:
            // findLabel("AttackT.end")` and then steps `sprite.frame = start
            // + counter` — so returning VOID here stalls them outright.
            "findlabel" => {
                let label = runtime.with_player_and_symbols(|player, symbols| {
                    if args.is_empty() {
                        return Err(ScriptError::new(
                            "findLabel requires a label name".to_string(),
                        ));
                    }
                    player.get_datum(&args[0]).string_value(symbols)
                })?;
                runtime.with_player_and_symbols(|player, symbols| {
                    let sprite_num = player.get_datum(datum).to_sprite_ref()?;
                    let frame = player
                        .movie
                        .score
                        .get_sprite(sprite_num)
                        .and_then(|s| s.member.as_ref())
                        .and_then(|m| player.movie.cast_manager.find_member_by_ref(m))
                        .and_then(|m| m.member_type.as_flash())
                        .map(|flash| {
                            crate::player::cast_member::CastMember::find_swf_frame_label(
                                &flash.data,
                                &label,
                            )
                        })
                        .unwrap_or(0);
                    Ok(player.alloc_datum(Datum::Int(frame as i32)))
                })
            }
            "telltarget" | "flashtostage" | "stagetoflash" => {
                warn!("Flash sprite method '{}' called but not yet implemented", handler_name);
                Ok(DatumRef::Void)
            }
            "newobject" => {
                // Flash sprite command: sprite(N).newObject(objectType {, args...})
                // Per the Director 11.5 Scripting Dictionary this "creates an
                // ActionScript object of the specified type" in the Flash movie and
                // returns a *reference* to it, for use with setCallback() / call() /
                // connect() etc (e.g. `sprite(3).newObject("LocalConnection")`).
                //
                // We return a sprite-bound FlashObjectRef with a unique synthetic
                // path — never VOID — mirroring getVariable()'s object form so the
                // chained setCallback()/connect() calls resolve. The callback bridge
                // (dirplayer_registerLingoCallbackOwned) and method dispatch
                // (dirplayer_ruffleCallFunction) key off this path. NOTE: extra
                // constructor args are not yet forwarded to a real AS constructor —
                // that needs a Ruffle-side newObject bridge to physically host the
                // object (e.g. so a LocalConnection can actually receive messages).
                let object_type = runtime.with_player_and_symbols(|player, symbols| {
                    if args.is_empty() {
                        return Err(ScriptError::new(
                            "newObject requires at least one argument".to_string(),
                        ));
                    }
                    player.get_datum(&args[0]).string_value(symbols)
                })?;
                runtime.with_player_and_symbols(|player, symbols| {
                    use crate::director::lingo::datum::FlashObjectRef;
                    let sprite_num = player.get_datum(datum).to_sprite_ref()?;
                    let (cl, cm) = player
                        .movie
                        .score
                        .get_sprite(sprite_num)
                        .and_then(|s| s.member.as_ref())
                        .map(|m| (m.cast_lib, m.cast_member))
                        .unwrap_or((0, 0));
                    let id = player.next_flash_object_id()?;
                    // Sanitise the class name into a readable, valid AS path segment.
                    let safe: String = object_type
                        .chars()
                        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
                        .collect();
                    let path = format!("_root.__dpObj_{}_{}", safe, id);
                    Ok(player.alloc_datum(Datum::FlashObjectRef(
                        FlashObjectRef::from_path_with_sprite(&path, cl, cm, sprite_num as i32),
                    )))
                })
            }
            "setscriptlist" => {
                // sprite.setScriptList(list)
                // list = [[memberRef, "propString"], [memberRef, "propString"], ...]
                // Creates new behavior instances from member refs and serialised
                // property lists, then replaces the sprite's script_instance_list.
                use super::script::ScriptDatumHandlers;
                use crate::player::ci_string::CiString;

                runtime.with_player_and_symbols(|player, symbols| {
                    let sprite_num = player.get_datum(datum).to_sprite_ref()?;
                    if args.is_empty() {
                        return Err(ScriptError::new("setScriptList requires 1 argument".to_string()));
                    }

                    let outer = player.get_datum(&args[0]).to_list()?
                        .iter().cloned().collect::<Vec<_>>();

                    // Collect (member_ref, props_str) pairs under the borrow
                    let mut pairs: Vec<(crate::player::cast_lib::CastMemberRef, String)> = Vec::new();
                    for pair_ref in &outer {
                        let pair = player.get_datum(pair_ref).to_list()?
                            .iter().cloned().collect::<Vec<_>>();
                        if pair.len() < 2 { continue; }
                        let member_ref = match player.get_datum(&pair[0]) {
                            Datum::CastMember(r) => r.clone(),
                            _ => continue,
                        };
                        let props_str = player.get_datum(&pair[1]).string_value(symbols).unwrap_or_default();
                        pairs.push((member_ref, props_str));
                    }
                    Ok((sprite_num, pairs))
                }).and_then(|(sprite_num, pairs)| {
                    // Create instances outside the player borrow using the existing
                    // ScriptDatumHandlers::create_script_instance helper.
                    let mut new_instances: Vec<crate::player::script_ref::ScriptInstanceRef> = Vec::new();

                    for (member_ref, props_str) in &pairs {
                        match ScriptDatumHandlers::create_script_instance(runtime.player, runtime.symbols, member_ref) {
                            Ok((instance_ref, _datum_ref)) => {
                                // Parse the property string and set properties on the instance
                                // via script_set_prop (standard Lingo property setter).
                                let trimmed = props_str.trim();
                                let inner = if trimmed.starts_with('[') && trimmed.ends_with(']') {
                                    &trimmed[1..trimmed.len()-1]
                                } else {
                                    trimmed
                                };
                                for part in inner.split(',') {
                                    let part = part.trim();
                                    if let Some((key, val)) = part.split_once(':') {
                                        let key = key.trim().trim_start_matches('#').to_string();
                                        let val = val.trim();
                                        let datum_val = if let Ok(i) = val.parse::<i32>() {
                                            Datum::Int(i)
                                        } else if let Ok(f) = val.parse::<f64>() {
                                            Datum::Float(f)
                                        } else if val.starts_with('"') && val.ends_with('"') {
                                            Datum::String(val[1..val.len()-1].to_string())
                                        } else if val.starts_with('#') {
                                            Datum::Symbol(runtime.symbols.intern(&val[1..]))
                                        } else {
                                            Datum::String(val.to_string())
                                        };
                                        let val_ref = runtime.player.alloc_datum(datum_val);
                                        let prop_name = runtime.symbols.intern(&key);
                                        let _ = crate::player::script::script_set_prop(
                                            runtime.player,
                                            runtime.symbols,
                                            &instance_ref,
                                            prop_name,
                                            &val_ref,
                                            false,
                                        );
                                    }
                                }
                                new_instances.push(instance_ref);
                            }
                            Err(e) => {
                                warn!(
                                    "[setScriptList] Failed to create instance for member({},{}): {}",
                                    member_ref.cast_lib, member_ref.cast_member, e.message
                                );
                            }
                        }
                    }

                    // Replace the sprite's behavior list
                    runtime.with_player_and_symbols(|player, symbols| {
                        let sprite = player.movie.score.get_sprite_mut(sprite_num);
                        sprite.script_instance_list = new_instances;
                        player.remove_script_instance_list_cache(sprite_num);
                        player.refresh_stage_behavior_channel_cache_entry(sprite_num);
                    });

                    Ok(DatumRef::Void)
                })
            }
            _ => Err(ScriptError::new_code(
                ScriptErrorCode::HandlerNotFound,
                format!("No sync handler {handler_name} for sprite"),
            )),
        }
    }


}
