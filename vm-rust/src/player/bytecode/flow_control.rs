use crate::{
    director::lingo::datum::Datum,
    player::{
        compare::datum_is_zero,
        datum_formatting::format_datum,
        datum_ref::DatumRef,
        handlers::datum_handlers::script_instance::ScriptInstanceUtils,
        scope::StackDatum,
        script::{get_current_handler_def, get_current_script},
        symbols::symbol::Symbol,
        HandlerExecutionResult, ScriptError,
    },
};

use super::handler_manager::BytecodeHandlerContext;

pub struct FlowControlBytecodeHandler {}

pub(crate) struct PreparedObjCall {
    pub(crate) receiver: DatumRef,
    pub(crate) name: Symbol,
    pub(crate) args: Vec<DatumRef>,
    pub(crate) push_return: bool,
    pub(crate) lingo_target: Option<(
        crate::player::ScriptInstanceRef,
        crate::player::script::ScriptHandlerRef,
    )>,
}

/// Decode ObjCall's stack payload once while the session owns its player.
/// Script-instance routing intentionally follows the legacy resolution order;
/// datum dispatch happens only after that route declines the call.
pub(crate) fn prepare_obj_call(
    runtime: &mut crate::player::session::ExecutionContext<'_>,
    ctx: &BytecodeHandlerContext,
) -> Result<PreparedObjCall, ScriptError> {
    runtime.with_player_and_symbols(|player, symbols| {
        let bytecode = player.get_ctx_current_bytecode(ctx);
        let name = ctx.get_name(bytecode.obj as u16);
        let name_text = symbols
            .display(&name)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        let (mut all_args, no_ret) = {
            let (scopes, allocator, bitmap_manager) =
                (&mut player.scopes, &mut player.allocator, &mut player.bitmap_manager);
            let scope = scopes.get_mut(ctx.scope_ref()).unwrap();
            let bytecode_index = scope.bytecode_index;
            match scope.pop_call_args(allocator, bitmap_manager) {
                Some(value) => value,
                None => {
                    let current_handler_name = ctx.get_name(scope.handler_name_id);
                    let current_handler_name = symbols
                        .display(&current_handler_name)
                        .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
                    return Err(ScriptError::new(format!(
                        "obj_call '{}': expected arg marker in handler '{}' (script={}:{}, scope_ref={}, bytecode_index={})",
                        name_text,
                        current_handler_name,
                        scope.script_ref.cast_lib,
                        scope.script_ref.cast_member,
                        ctx.scope_ref(),
                        bytecode_index
                    )));
                }
            }
        };
        if all_args.is_empty() {
            return Err(ScriptError::new(format!(
                "obj_call '{}': arg list has no receiver",
                name_text
            )));
        }
        let receiver = all_args.remove(0);
        let args = all_args;
        let lingo_target = match &receiver {
            DatumRef::Void => None,
            _ => match player.allocator.try_get_datum(&receiver) {
                Some(Datum::ScriptInstanceRef(instance_ref)) => {
                    let instance_ref = instance_ref.clone();
                    if crate::player::virtual_scripts::VirtualScriptRegistry::has_instance_handler(
                        player,
                        symbols,
                        &instance_ref,
                        name.clone(),
                    )? {
                        None
                    } else {
                        ScriptInstanceUtils::get_handler(name.clone(), &receiver, player)?
                            .map(|handler_ref| (instance_ref, handler_ref))
                    }
                }
                Some(_) => None,
                None => {
                    return Err(ScriptError::new(format!(
                        "invalid datum reference {receiver}"
                    )));
                }
            },
        };
        Ok(PreparedObjCall {
            receiver,
            name,
            args,
            push_return: !no_ret,
            lingo_target,
        })
    })
}

impl FlowControlBytecodeHandler {
    pub fn ret(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player(|player| {
            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            scope.return_value = DatumRef::Void;
            scope.stack.clear();
        });
        Ok(HandlerExecutionResult::Stop)
    }

    /// `tell <target>` — pop the target and record what the enclosed statements
    /// re-point at. `tell sprite(#movieSprite)` targets that nested sub-player
    /// (the loader→game command bridge); `tell sprite(<film loop>)` re-points
    /// score reads at the film loop's own playhead; other targets run on THIS
    /// player. Stack is a Vec so `tell` blocks can nest.
    pub fn start_tell(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player(|player| {
            let target_ref = {
                let (scopes, allocator, bitmap_manager) = (
                    &mut player.scopes,
                    &mut player.allocator,
                    &mut player.bitmap_manager,
                );
                let scope = scopes.get_mut(ctx.scope_ref()).unwrap();
                scope.stack.pop_ref_with(allocator, bitmap_manager)
            }
            .ok_or_else(|| ScriptError::new("starttell: operand stack is empty".to_string()))?;
            let target = player.get_datum(&target_ref).clone();
            // Resolve the target to the member it is showing; a sprite tells
            // through to its member, a member ref is already one.
            let member_ref = match target {
                Datum::SpriteRef(n) => player
                    .movie
                    .score
                    .get_sprite(n as i16)
                    .and_then(|s| s.member.clone()),
                Datum::CastMember(m) => Some(m),
                _ => None,
            };
            let tell_target = match member_ref {
                Some(m) => crate::player::TellTarget {
                    nested_player: crate::player::nested_player_id(&m),
                    film_loop: player
                        .movie
                        .cast_manager
                        .find_member_by_ref(&m)
                        .filter(|member| {
                            matches!(
                                member.member_type,
                                crate::player::cast_member::CastMemberType::FilmLoop(_)
                            )
                        })
                        .map(|_| m),
                },
                None => crate::player::TellTarget::default(),
            };
            player.tell_target_stack.push(tell_target);
            Ok(HandlerExecutionResult::Advance)
        })
    }

    /// `end tell` — pop the current tell target.
    pub fn end_tell(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        let _ = ctx;
        runtime.with_player(|player| {
            player.tell_target_stack.pop();
        });
        Ok(HandlerExecutionResult::Advance)
    }

    pub fn local_call(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let (args, is_no_ret) = {
                let (scopes, allocator, bitmap_manager) =
                    (&mut player.scopes, &mut player.allocator, &mut player.bitmap_manager);
                let scope = scopes.get_mut(ctx.scope_ref()).unwrap();
                match scope.pop_call_args(allocator, bitmap_manager) {
                    Some(v) => v,
                    None => {
                        let current_handler_name = ctx.get_name(scope.handler_name_id);
                        let current_handler_name = symbols
                            .display(&current_handler_name)
                            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
                        return Err(ScriptError::new(format!(
                            "local_call: expected arg marker in handler '{}' (script={}:{}, scope_ref={}, bytecode_index={})",
                            current_handler_name, scope.script_ref.cast_lib, scope.script_ref.cast_member, ctx.scope_ref(), scope.bytecode_index
                        )));
                    }
                }
            };

            let script = get_current_script(&ctx);

            let handler_index = player.get_ctx_current_bytecode(&ctx).obj as usize;
            let mut handler_ref = match script.get_own_handler_ref_at(handler_index) {
                Some(h) => h,
                None => {
                    return Err(ScriptError::new(format!(
                        "local_call: no own handler at index {} (script={}:{}, has {} handlers)",
                        handler_index,
                        script.member_ref.cast_lib,
                        script.member_ref.cast_member,
                        script.handler_names.len()
                    )));
                }
            };
            let handler_name = handler_ref.1.clone();

            // if first arg is a script or script instance and has a handler by the same name
            // use that handler instead
            let mut receiver;
            if let Some(handler_pair) =
                ScriptInstanceUtils::get_handler_from_first_arg(
                    player,
                    symbols,
                    &args,
                    &handler_name,
                )?
            {
                receiver = handler_pair.0;
                handler_ref = handler_pair.1;
            } else {
                let scope = player.scopes.get(ctx.scope_ref()).unwrap();
                receiver = scope.receiver.clone();
            }
            // Hand the call to the trampoline driver instead of recursively awaiting
            // (which would box a future). `use_raw_arg_list: true` matches the old
            // `player_call_script_handler_raw_args(.., true)` call; the driver does
            // `player_handle_scope_return` + the result push on the callee's return.
            Ok(HandlerExecutionResult::Call(crate::player::PendingCall {
                receiver,
                handler_ref,
                args,
                use_raw_arg_list: true,
                push_return: !is_no_ret,
            }))
        })
    }

    pub fn jmp_if_zero(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            // Inline-aware: an int/void condition (the overwhelming common case
            // for loop/if guards) is tested directly without materializing a
            // DatumRef or touching the arena.
            let popped = player.scopes.get_mut(ctx.scope_ref()).unwrap().stack.pop_value();
            let is_zero = match popped {
                Some(StackDatum::Int(n)) => n == 0,
                Some(StackDatum::Void) => true,
                Some(other) => {
                    let value_id = other.into_ref_with(&mut player.allocator, &mut player.bitmap_manager);
                    datum_is_zero(player.get_datum(&value_id), &player.allocator, symbols)?
                }
                None => {
                    let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                    let current_handler_name = symbols
                        .display(&ctx.get_name(scope.handler_name_id))
                        .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
                    return Err(ScriptError::new(format!(
                        "jmp_if_zero: stack underflow in handler '{}' (script={}:{}, scope_ref={}, bytecode_index={})",
                        current_handler_name, scope.script_ref.cast_lib, scope.script_ref.cast_member, ctx.scope_ref(), scope.bytecode_index
                    )));
                }
            };

            let bytecode = player.get_ctx_current_bytecode(&ctx);
            let position = bytecode.pos as i32;
            let offset = bytecode.obj as i32;

            if is_zero {
                let new_bytecode_index = {
                    let handler = get_current_handler_def(&ctx);
                    let dest_pos = (position as i32 + offset) as usize;
                    handler.bytecode_index_map[&dest_pos] as usize
                };
                let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                scope.bytecode_index = new_bytecode_index;
                Ok(HandlerExecutionResult::Jump)
            } else {
                Ok(HandlerExecutionResult::Advance)
            }
        })
    }

    pub fn jmp(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player(|player| {
            let bytecode = player.get_ctx_current_bytecode(ctx);
            let new_bytecode_index = {
                let handler = get_current_handler_def(&ctx);
                let dest_pos = (bytecode.pos as i32 + bytecode.obj as i32) as usize;
                handler.bytecode_index_map[&dest_pos] as usize
            };
            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            scope.bytecode_index = new_bytecode_index;
            Ok(HandlerExecutionResult::Jump)
        })
    }

    pub fn end_repeat(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player(|player| {
            let new_index = {
                let bytecode = player.get_ctx_current_bytecode(ctx);
                let handler = get_current_handler_def(&ctx);
                let return_pos = bytecode.pos - bytecode.obj as usize;
                handler.bytecode_index_map[&return_pos] as usize
            };
            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            scope.bytecode_index = new_index;
            Ok(HandlerExecutionResult::Jump)
        })
    }

    pub fn call_javascript(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let (arg1, arg2) = {
                let (scopes, allocator, bitmap_manager) = (
                    &mut player.scopes,
                    &mut player.allocator,
                    &mut player.bitmap_manager,
                );
                let scope = scopes.get_mut(ctx.scope_ref()).unwrap();
                let arg1 = scope
                    .stack
                    .pop_ref_with(allocator, bitmap_manager)
                    .ok_or_else(|| {
                        ScriptError::new("call_javascript: stack underflow (arg1)".to_string())
                    })?;
                let arg2 = scope
                    .stack
                    .pop_ref_with(allocator, bitmap_manager)
                    .ok_or_else(|| {
                        ScriptError::new("call_javascript: stack underflow (arg2)".to_string())
                    })?;
                (arg1, arg2)
            };
            let arg1_formatted = format_datum(&arg1, symbols, player)?;
            let arg2_formatted = format_datum(&arg2, symbols, player)?;

            log::warn!(
                "TODO: call_javascript with args: {}, {}",
                arg1_formatted,
                arg2_formatted
            );
            Ok(HandlerExecutionResult::Advance)
        })
    }
}
