use std::collections::VecDeque;
use crate::{
    director::lingo::datum::{Datum, DatumType},
    player::{
        DatumRef, HandlerExecutionResult, ScriptError, context_vars::player_get_context_var
    },
};

use super::handler_manager::BytecodeHandlerContext;

pub struct StackBytecodeHandler {}

impl StackBytecodeHandler {
    pub fn push_int(runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player(|player| {
            let n = player.get_ctx_current_bytecode(ctx).obj as i32;
            // Inline: store the int directly on the stack — no Datum, no DatumRef,
            // no arena. Materialized to a DatumRef only if/when a consumer needs one.
            player.scopes.get_mut(ctx.scope_ref()).unwrap().stack.push_int(n);
        });
        Ok(HandlerExecutionResult::Advance)
    }

    pub fn push_f32(runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player(|player| {
            let obj_value = player.get_ctx_current_bytecode(ctx).obj as u32;
            
            // Interpret the 32 bits as f32, THEN convert to f64
            let float_f32 = f32::from_bits(obj_value);
            let float_f64 = float_f32 as f64;
            
            player.scopes.get_mut(ctx.scope_ref()).unwrap().stack.push_float(float_f64);
        });
        Ok(HandlerExecutionResult::Advance)
    }

    /// Mark the top `n` stack entries as a call's arguments.
    ///
    /// This used to drain them into a `VecDeque` and allocate a
    /// `Datum::List(ArgList, ..)` — a heap allocation plus an arena allocation —
    /// for the sole benefit of the call opcode that immediately follows, which
    /// took the list straight back apart. `pusharglistnoret` and `pusharglist`
    /// together are ~17% of Agent Free Ride's profile and the most expensive ops
    /// in the interpreter bench (85-91 ns/op). Leave the arguments where they
    /// are and push a marker instead: no allocation at all.
    pub fn push_arglist(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        Self::push_arg_marker(runtime, ctx, false)
    }

    fn push_arg_marker(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
        no_ret: bool,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player(|player| {
            let count = player.get_ctx_current_bytecode(ctx).obj as usize;
            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            if scope.stack.len() < count {
                return Err(ScriptError::new(
                    "Not enough items in stack to create arglist".to_string(),
                ));
            }
            scope.stack.push_value(crate::player::scope::StackDatum::ArgMarker {
                count: count as u16,
                no_ret,
            });
            Ok(())
        })?;
        Ok(HandlerExecutionResult::Advance)
    }

    /// As `push_arglist`, for a call whose result is discarded.
    pub fn push_arglist_no_ret(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        Self::push_arg_marker(runtime, ctx, true)
    }

    pub fn push_symb(runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext) -> Result<HandlerExecutionResult, ScriptError> {
        let player = &mut *runtime.player;
        let name_id = player.get_ctx_current_bytecode(ctx).obj;
        // ctx.get_name indexes the retained cast snapshot — no per-op get_cast lookup
        // The retained cast snapshot avoids re-resolving the cast's name table.
        let symbol_name = ctx.get_name(name_id as u16);
        let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
        scope.stack.push_symbol(symbol_name);
        Ok(HandlerExecutionResult::Advance)
    }

    pub fn push_var_ref(runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext) -> Result<HandlerExecutionResult, ScriptError> {
        let player = &mut *runtime.player;
        let name_id = player.get_ctx_current_bytecode(ctx).obj;
        let symbol_name = ctx.get_name(name_id as u16);
        let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
        scope.stack.push_symbol(symbol_name);
        Ok(HandlerExecutionResult::Advance)
    }

    pub fn push_cons(runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player(|player| {
            let literal_id = (player.get_ctx_current_bytecode(ctx).obj as u32 / ctx.multiplier) as usize;
            // The retained script is valid for the whole handler and lets us
            // call the &mut player fast-path allocators below.
            let script = ctx.code.script.as_ref();
            let literal = &script.chunk.literals[literal_id];
            // Fast paths: int/symbol literals build the DatumRef directly (no
            // 64-byte Datum clone + move). Other literal types clone as before.
            // Inline primitive literals (no Datum clone/alloc); other literal
            // types allocate and push a ref as before.
            match literal {
                Datum::Int(n) => { let n = *n; player.scopes.get_mut(ctx.scope_ref()).unwrap().stack.push_int(n); }
                Datum::Float(f) => { let f = *f; player.scopes.get_mut(ctx.scope_ref()).unwrap().stack.push_float(f); }
                Datum::Symbol(s) => { let s = s.clone(); player.scopes.get_mut(ctx.scope_ref()).unwrap().stack.push_symbol(s); }
                Datum::Void => { player.scopes.get_mut(ctx.scope_ref()).unwrap().stack.push_void(); }
                other => {
                    let dr = player.alloc_datum(other.clone());
                    player.scopes.get_mut(ctx.scope_ref()).unwrap().stack.push(dr);
                }
            }
            Ok(HandlerExecutionResult::Advance)
        })
    }

    pub fn push_zero(runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player(|player| {
            player.scopes.get_mut(ctx.scope_ref()).unwrap().stack.push_int(0);
            Ok(HandlerExecutionResult::Advance)
        })
    }

    pub fn push_prop_list(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        // `[#a: 1, #b: 2]` compiles to `pusharglist N` + `pushproplist`, so the
        // key/value pairs arrive as a call-style arg marker.
        let arg_list: Vec<DatumRef> = runtime.with_player(|player| {
            let (scopes, allocator, bitmap_manager) =
                (&mut player.scopes, &mut player.allocator, &mut player.bitmap_manager);
            let scope = scopes.get_mut(ctx.scope_ref()).unwrap();
            scope.pop_call_args(allocator, bitmap_manager).map(|(a, _)| a).ok_or_else(|| {
                ScriptError::new("push_prop_list: expected arg marker on stack".to_string())
            })
        })?;
        runtime.with_player(|player| {
            if arg_list.len() % 2 != 0 {
                return Err(ScriptError::new("argList length must be even".to_string()));
            }
            let entry_count = arg_list.len() / 2;
            let entries = (0..entry_count)
                .map(|index| {
                    let base_index = index * 2;
                    let key = arg_list[base_index].to_owned();
                    let value = arg_list[base_index + 1].to_owned();
                    (key, value)
                })
                .collect::<VecDeque<(DatumRef, DatumRef)>>();
            let datum_ref = player.alloc_datum(Datum::PropList(entries, false));
            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            scope.stack.push(datum_ref);
            Ok(HandlerExecutionResult::Advance)
        })
    }

    pub fn push_list(runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player(|player| {
            // `[a, b, c]` compiles to `pusharglist N` + `pushlist`, so the
            // elements arrive as a call-style arg marker, not a list datum.
            let (items, _) = {
                let (scopes, allocator, bitmap_manager) =
                    (&mut player.scopes, &mut player.allocator, &mut player.bitmap_manager);
                let scope = scopes.get_mut(ctx.scope_ref()).unwrap();
                scope.pop_call_args(allocator, bitmap_manager).ok_or_else(|| {
                    ScriptError::new("push_list: expected arg marker on stack".to_string())
                })?
            };
            let list: VecDeque<DatumRef> = items.into();
            let result_id = player.alloc_datum(Datum::List(DatumType::List, list, false));
            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            scope.stack.push(result_id);
            Ok(HandlerExecutionResult::Advance)
        })
    }

    pub fn peek(runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player(|player| {
            let offset = player.get_ctx_current_bytecode(ctx).obj;
            let datum_ref = {
                let (scopes, allocator, bitmap_manager) =
                    (&mut player.scopes, &mut player.allocator, &mut player.bitmap_manager);
                let scope = scopes.get_mut(ctx.scope_ref()).unwrap();
                let stack_index = scope.stack.len() - 1 - offset as usize;
                scope.stack.get_ref_with(stack_index, allocator, bitmap_manager).unwrap()
            };
            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            scope.stack.push(datum_ref);
            Ok(HandlerExecutionResult::Advance)
        })
    }

    pub fn pop(runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player(|player| {
            let count = player.get_ctx_current_bytecode(ctx).obj;
            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            // The Pop opcode throws the values away — discard inline entries
            // without an alloc_int materialization round-trip.
            scope.stack.discard(count as usize);
        });
        Ok(HandlerExecutionResult::Advance)
    }

    pub fn push_chunk_var_ref(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        // PushChunkVarRef uses RAW (non-multiplied) variable indices,
        // unlike getparam/setparam/deletechunk which use multiplied indices.
        // e.g. for handler(me, t): deletechunk uses pushint8 8 for t (8/8=1),
        // but pushchunkvarref uses pushint8 1 for t (raw index 1).
        runtime.with_player_and_symbols(|player, symbols| {
            if !ctx.scope.validate_top(player) {
                return Err(crate::player::cancelled_scope_error());
            }
            let var_type = player.get_ctx_current_bytecode(ctx).obj as u32;
            let id_ref = {
                let (scopes, allocator, bitmap_manager) =
                    (&mut player.scopes, &mut player.allocator, &mut player.bitmap_manager);
                let scope = scopes.get_mut(ctx.scope_ref()).unwrap();
                scope.stack.pop_ref_with(allocator, bitmap_manager).unwrap()
            };
            let id = player.get_datum(&id_ref).int_value()?;

            let value_ref = match var_type {
                0x4 => {
                    // argument - raw index, no variable multiplier
                    let arg_index = id as usize;
                    let scope = player.scopes.get(ctx.scope_ref()).unwrap();
                    scope.args.get(arg_index).cloned().unwrap_or(DatumRef::Void)
                }
                0x5 => {
                    // local - raw index, no variable multiplier. `id` is the
                    // dense slot directly.
                    let local = player.scopes.get(ctx.scope_ref()).unwrap().local(id as usize);
                    local.into_ref_with(
                        &mut player.allocator,
                        &mut player.bitmap_manager,
                    )
                }
                _ => {
                    // For other var types (field etc.), fall back to the standard path
                    let cast_id_ref = if var_type == 0x6 && player.movie.dir_version >= 500 {
                        let (scopes, allocator, bitmap_manager) =
                            (&mut player.scopes, &mut player.allocator, &mut player.bitmap_manager);
                        let scope = scopes.get_mut(ctx.scope_ref()).unwrap();
                        Some(scope.stack.pop_ref_with(allocator, bitmap_manager).unwrap())
                    } else {
                        None
                    };
                    player_get_context_var(
                        player,
                        &id_ref,
                        cast_id_ref.as_ref(),
                        var_type,
                        ctx,
                        symbols,
                    )?
                }
            };

            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            scope.stack.push(value_ref);
            Ok(HandlerExecutionResult::Advance)
        })
    }

    pub fn swap(runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player(|player| {
            let (scopes, allocator, bitmap_manager) =
                (&mut player.scopes, &mut player.allocator, &mut player.bitmap_manager);
            let scope = scopes.get_mut(ctx.scope_ref()).unwrap();
            let a = scope.stack.pop_ref_with(allocator, bitmap_manager).unwrap();
            let b = scope.stack.pop_ref_with(allocator, bitmap_manager).unwrap();
            scope.stack.push(a);
            scope.stack.push(b);
            Ok(HandlerExecutionResult::Advance)
        })
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::HashMap, rc::Rc};

    use crate::{
        director::{
            chunks::{handler::{Bytecode, HandlerDef}, script::ScriptChunk},
            enums::ScriptType,
            lingo::opcode::OpCode,
        },
        player::{
            bytecode::handler_manager::{BytecodeHandlerContext, HandlerCode, StaticBytecodeHandlerManager},
            cast_lib::CastMemberRef,
            ownership::{OwnerKey, OwnerToken},
            scope::{ScopeRef, StackDatum},
            script::Script,
            session::ExecutionContext,
            symbols::symbol::{Symbol},
            symbols::symbol_table::SymbolTable,
            ScopeToken, DirPlayer, ScriptErrorCode,
        },
    };

    fn make_player() -> DirPlayer {
        let (tx, _rx) = async_std::channel::unbounded();
        DirPlayer::new_with_owner(
            tx,
            OwnerToken::new(OwnerKey { session: 73, player: 1, generation: 1 }),
        )
    }

    fn make_context(
        player: &DirPlayer,
        slot: ScopeRef,
        opcode: OpCode,
        obj: i64,
    ) -> BytecodeHandlerContext {
        let handler = Rc::new(HandlerDef {
            name_id: 0,
            bytecode_array: vec![Bytecode::new(opcode, obj, 0)],
            bytecode_index_map: fxhash::FxHashMap::default(),
            argument_name_ids: vec![],
            local_name_ids: vec![],
            global_name_ids: vec![],
            compiled_ir: RefCell::new(None),
        });
        let script = Rc::new(Script {
            member_ref: CastMemberRef { cast_lib: 0, cast_member: 0 },
            name: String::new(),
            chunk: ScriptChunk {
                script_number: 0,
                literals: vec![],
                handlers: vec![],
                property_name_ids: vec![],
                property_defaults: HashMap::new(),
            },
            script_type: ScriptType::Movie,
            handlers: fxhash::FxHashMap::default(),
            handler_names_raw: vec![],
            handler_names: vec![],
            properties: RefCell::new(fxhash::FxHashMap::default()),
        });
        let scope = &player.scopes[slot];
        BytecodeHandlerContext {
            scope: ScopeToken {
                owner: player.owner.clone(),
                slot,
                generation: scope.generation,
                epoch: player.scope_invalidation_epoch,
            },
            code: HandlerCode {
                script,
                handler,
                names: Rc::from(Vec::<Symbol>::new()),
            },
            multiplier: 1,
        }
    }

    #[test]
    fn push_chunk_var_ref_passes_session_symbols_to_field_lookup() {
        let mut player = make_player();
        player.movie.dir_version = 500;
        let slot = player.push_scope();
        let id_ref = player.alloc_datum(Datum::Int(1));
        let mut foreign_symbols = SymbolTable::new();
        let foreign_cast_ref = player.alloc_datum(Datum::Symbol(
            foreign_symbols.intern("foreignCast"),
        ));
        player.scopes[slot].stack.push(foreign_cast_ref);
        player.scopes[slot].stack.push(id_ref);
        let ctx = make_context(&player, slot, OpCode::PushChunkVarRef, 0x6);
        let mut symbols = SymbolTable::new();
        let result = {
            let mut runtime = ExecutionContext {
                player_id: 1,
                symbols: &mut symbols,
                player: &mut player,
            };
            StaticBytecodeHandlerManager::call_sync_handler(
                OpCode::PushChunkVarRef,
                &mut runtime,
                &ctx,
            )
        };
        assert!(result.is_err());
        assert!(player.scopes[slot].stack.is_empty());
        player.pop_scope();
    }

    #[test]
    fn push_chunk_var_ref_rejects_stale_scope_before_popping() {
        let mut player = make_player();
        let slot = player.push_scope();
        let ctx = make_context(&player, slot, OpCode::PushChunkVarRef, 0x1);
        player.pop_scope();
        let reused = player.push_scope();
        assert_eq!(reused, slot);
        player.scopes[slot].stack.push_value(StackDatum::Int(7));
        let mut symbols = SymbolTable::new();
        let result = {
            let mut runtime = ExecutionContext {
                player_id: 1,
                symbols: &mut symbols,
                player: &mut player,
            };
            StackBytecodeHandler::push_chunk_var_ref(&mut runtime, &ctx)
        };
        assert_eq!(result.err().unwrap().code, ScriptErrorCode::Abort);
        assert_eq!(player.scopes[slot].stack.len(), 1);
        player.pop_scope();
    }
}
