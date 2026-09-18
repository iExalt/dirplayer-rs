use crate::{
    director::lingo::datum::{
        datum_bool, Datum, StringChunkExpr, StringChunkSource, StringChunkType,
    },
    player::{
        context_vars::{player_get_context_var, player_set_context_var, read_context_var_args},
        datum_formatting::{datum_to_string_for_concat, format_concrete_datum},
        datum_ref::DatumRef,
        handlers::datum_handlers::string_chunk::StringChunkUtils,
        symbols::symbol_table::SymbolTable,
        DirPlayer, HandlerExecutionResult, ScriptError,
    },
};

use super::handler_manager::BytecodeHandlerContext;

pub enum PutType {
    Into,
    After,
    Before,
}

impl From<u8> for PutType {
    fn from(val: u8) -> Self {
        match val {
            0x01 => PutType::Into,
            0x02 => PutType::After,
            0x03 => PutType::Before,
            _ => panic!("Invalid put type"),
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    use std::{cell::RefCell, collections::HashMap, rc::Rc};

    use crate::director::{
        chunks::{
            handler::{Bytecode, HandlerDef},
            script::ScriptChunk,
        },
        enums::ScriptType,
        lingo::opcode::OpCode,
    };
    use crate::player::{
        bytecode::handler_manager::{BytecodeHandlerContext, HandlerCode},
        cast_lib::CastMemberRef,
        ownership::{OwnerKey, OwnerToken},
        scope::{ScopeRef, StackDatum},
        script::Script,
        symbols::symbol_table::SymbolTable,
        ScopeToken,
    };

    fn make_player() -> DirPlayer {
        let (tx, _rx) = async_std::channel::unbounded();
        DirPlayer::new_with_owner(
            tx,
            OwnerToken::new(OwnerKey {
                session: 29,
                player: 6,
                generation: 1,
            }),
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
            member_ref: CastMemberRef {
                cast_lib: 0,
                cast_member: 0,
            },
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
                names: Rc::from(Vec::<crate::player::symbols::symbol::Symbol>::new()),
            },
            multiplier: 1,
        }
    }

    fn push_put_chunk_stack(
        player: &mut DirPlayer,
        slot: ScopeRef,
        value_ref: DatumRef,
        id_ref: DatumRef,
        cast_id_ref: Option<DatumRef>,
    ) {
        let scope = player.scopes.get_mut(slot).unwrap();
        scope.stack.push_value(StackDatum::Ref(value_ref));
        // read_single_chunk_ref pops these from last_line through first_char.
        for value in [1, 1, 0, 0, 0, 0, 0, 0] {
            scope.stack.push_value(StackDatum::Int(value));
        }
        scope.stack.push_value(StackDatum::Ref(id_ref));
        if let Some(cast_id_ref) = cast_id_ref {
            scope.stack.push_value(StackDatum::Ref(cast_id_ref));
        }
    }

    #[test]
    fn custom_concat_keeps_null_distinct_from_canonical_join_and_preserves_symbol_spelling() {
        let mut player = make_player();
        let mut symbols = SymbolTable::new();
        let null = player.alloc_datum(Datum::Null);
        let suffix = player.alloc_datum(Datum::String("tail".to_owned()));
        let custom = StringBytecodeHandler::concat_datums(
            null.clone(),
            suffix.clone(),
            &mut player,
            &symbols,
            false,
        )
        .unwrap();
        assert!(matches!(player.get_datum(&custom), Datum::String(value) if value == "<Null>tail"));
        let padded = StringBytecodeHandler::concat_datums(
            null.clone(),
            suffix.clone(),
            &mut player,
            &symbols,
            true,
        )
        .unwrap();
        assert!(
            matches!(player.get_datum(&padded), Datum::String(value) if value == "<Null> tail")
        );
        let canonical =
            datum_to_string_for_concat(player.get_datum(&null), &symbols, &player).unwrap();
        assert_eq!(format!("{canonical}tail"), "tail");

        let symbol = symbols.intern_authoritative("MiXeDKey");
        let symbol_ref = player.alloc_datum(Datum::Symbol(symbol));
        let custom = StringBytecodeHandler::concat_datums(
            symbol_ref,
            suffix.clone(),
            &mut player,
            &symbols,
            false,
        )
        .unwrap();
        assert!(
            matches!(player.get_datum(&custom), Datum::String(value) if value == "MiXeDKeytail")
        );

        let mut foreign_table = SymbolTable::new();
        let foreign = player.alloc_datum(Datum::Symbol(foreign_table.intern("foreignConcat")));
        assert!(StringBytecodeHandler::concat_datums(
            foreign,
            suffix,
            &mut player,
            &symbols,
            false,
        )
        .is_err());
    }

    #[test]
    fn join_str_opcode_uses_canonical_null_conversion() {
        let mut player = make_player();
        let mut symbols = SymbolTable::new();
        let slot = player.push_scope();
        let left = player.alloc_datum(Datum::Null);
        let right = player.alloc_datum(Datum::String("tail".to_owned()));
        {
            let scope = player.scopes.get_mut(slot).unwrap();
            scope.stack.push_value(StackDatum::Ref(left));
            scope.stack.push_value(StackDatum::Ref(right));
        }
        let ctx = make_context(&player, slot, OpCode::JoinStr, 0);
        let result = {
            let mut runtime = crate::player::session::ExecutionContext {
                player_id: 6,
                symbols: &mut symbols,
                player: &mut player,
            };
            StringBytecodeHandler::join_str(&mut runtime, &ctx).unwrap()
        };
        assert!(matches!(result, HandlerExecutionResult::Advance));
        let result_ref = player
            .scopes
            .get_mut(slot)
            .unwrap()
            .stack
            .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
            .unwrap();
        assert!(matches!(player.get_datum(&result_ref), Datum::String(value) if value == "tail"));
        player.pop_scope();
    }

    #[test]
    fn put_chunk_rejects_foreign_field_operands_before_swallowing_lookup_errors() {
        let mut player = make_player();
        let mut symbols = SymbolTable::new();
        let mut foreign_table = SymbolTable::new();

        // A foreign field VALUE must fail before the field lookup's ordinary
        // missing-member error is swallowed by put_chunk.
        let value = player.alloc_datum(Datum::Symbol(foreign_table.intern("foreignValue")));
        let id = player.alloc_datum(Datum::Int(1));
        let slot = player.push_scope();
        push_put_chunk_stack(&mut player, slot, value, id, None);
        let ctx = make_context(&player, slot, OpCode::PutChunk, 0x16);
        let result = {
            let mut runtime = crate::player::session::ExecutionContext {
                player_id: 6,
                symbols: &mut symbols,
                player: &mut player,
            };
            StringBytecodeHandler::put_chunk(&mut runtime, &ctx)
        };
        assert!(result.is_err());
        player.pop_scope();

        // With a cast identifier, the same preflight must reject a foreign
        // cast Symbol before find_member_ref_by_identifiers/logging.
        player.movie.dir_version = 500;
        let value = player.alloc_datum(Datum::String("value".to_owned()));
        let id = player.alloc_datum(Datum::Int(1));
        let cast_id = player.alloc_datum(Datum::Symbol(foreign_table.intern("foreignCast")));
        let slot = player.push_scope();
        push_put_chunk_stack(&mut player, slot, value, id, Some(cast_id));
        let ctx = make_context(&player, slot, OpCode::PutChunk, 0x16);
        let result = {
            let mut runtime = crate::player::session::ExecutionContext {
                player_id: 6,
                symbols: &mut symbols,
                player: &mut player,
            };
            StringBytecodeHandler::put_chunk(&mut runtime, &ctx)
        };
        assert!(result.is_err());
        player.pop_scope();

        // An identifier Symbol is also validated independently of the cast.
        let value = player.alloc_datum(Datum::String("value".to_owned()));
        let id = player.alloc_datum(Datum::Symbol(foreign_table.intern("foreignId")));
        // dir_version >= 500 consumes a cast identifier before the member
        // identifier. Keep this stack layout valid so the foreign id reaches
        // the ownership check instead of an unrelated stack pop.
        let cast_id = player.alloc_datum(Datum::Int(1));
        let slot = player.push_scope();
        push_put_chunk_stack(&mut player, slot, value, id, Some(cast_id));
        let ctx = make_context(&player, slot, OpCode::PutChunk, 0x16);
        let result = {
            let mut runtime = crate::player::session::ExecutionContext {
                player_id: 6,
                symbols: &mut symbols,
                player: &mut player,
            };
            StringBytecodeHandler::put_chunk(&mut runtime, &ctx)
        };
        assert!(result.is_err());
        player.pop_scope();
    }
}

pub struct StringBytecodeHandler {}

impl StringBytecodeHandler {
    fn get_datum_concat_value(
        datum: &Datum,
        player: &DirPlayer,
        symbols: &SymbolTable,
    ) -> Result<String, ScriptError> {
        match datum {
            Datum::String(s) => Ok(s.clone()),
            Datum::StringChunk(..) => datum.string_value(symbols),
            Datum::Int(i) => Ok(i.to_string()),
            Datum::Symbol(s) => Ok(symbols
                .display(s)
                .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
                .to_owned()),
            Datum::Void => Ok("".to_string()),
            _ => format_concrete_datum(datum, symbols, player),
        }
    }

    pub fn concat_datums(
        left: DatumRef,
        right: DatumRef,
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        pad: bool,
    ) -> Result<DatumRef, ScriptError> {
        let right = player.get_datum(&right);
        let left = player.get_datum(&left);

        let right = Self::get_datum_concat_value(right, player, symbols)?;
        let left = Self::get_datum_concat_value(left, player, symbols)?;

        let result = if pad {
            format!("{} {}", left, right)
        } else {
            format!("{}{}", left, right)
        };
        Ok(player.alloc_datum(Datum::String(result)))
    }

    pub fn contains_str(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let (search_in, search_str) = {
                let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                let search_str = scope
                    .stack
                    .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                    .unwrap();
                let search_in = scope
                    .stack
                    .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                    .unwrap();
                (search_in, search_str)
            };
            let search_str = player.get_datum(&search_str).string_value(symbols)?;
            let search_in = player.get_datum(&search_in);

            // Director's `contains` operator is case-insensitive
            let search_str_lower = search_str.to_ascii_lowercase();
            let contains = if search_in.is_list() {
                let search_list = search_in.to_list()?;
                let mut contains = false;
                for item in search_list {
                    let item = player.get_datum(item);
                    if item.is_string() {
                        let item = item.string_value(symbols)?;
                        if item
                            .to_ascii_lowercase()
                            .contains(search_str_lower.as_str())
                        {
                            contains = true;
                            break;
                        }
                    }
                }
                Ok(contains)
            } else if search_in.is_string() {
                let search_in = search_in.string_value(symbols)?;
                Ok(search_in
                    .to_ascii_lowercase()
                    .contains(search_str_lower.as_str()))
            } else if search_in.is_symbol() {
                Ok(false)
            } else if search_in.is_number() {
                Ok(false)
            } else if search_in.is_void() {
                Ok(false)
            } else {
                Err(ScriptError::new(
                    "kOpContainsStr invalid search subject".to_string(),
                ))
            }?;

            let result_id = player.alloc_datum(datum_bool(contains));
            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            scope.stack.push(result_id);
            Ok(HandlerExecutionResult::Advance)
        })
    }

    pub fn join_pad_str(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let (left_id, right_id) = {
                let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                let right = scope
                    .stack
                    .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                    .unwrap();
                let left = scope
                    .stack
                    .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                    .unwrap();
                (left, right)
            };

            let result_id = Self::concat_datums(left_id, right_id, player, symbols, true)?;
            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            scope.stack.push(result_id);
            Ok(HandlerExecutionResult::Advance)
        })
    }

    pub fn join_str(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let (left_ref, right_ref) = {
                let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                let right_ref = scope
                    .stack
                    .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                    .unwrap();
                let left_ref = scope
                    .stack
                    .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                    .unwrap();
                (left_ref, right_ref)
            };

            // Get the actual datums
            let left = player.get_datum(&left_ref);
            let right = player.get_datum(&right_ref);

            let left_str = datum_to_string_for_concat(left, symbols, player)?;
            let right_str = datum_to_string_for_concat(right, symbols, player)?;

            // Concatenate
            let result = Datum::String(format!("{}{}", left_str, right_str));
            let result_ref = player.alloc_datum(result);

            // Push result back to stack
            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            scope.stack.push(result_ref);

            Ok(HandlerExecutionResult::Advance)
        })
    }

    pub fn put(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let bytecode = player.get_ctx_current_bytecode(ctx);
            let put_type = PutType::from(((bytecode.obj >> 4) & 0xF) as u8);
            let var_type = (bytecode.obj & 0xF) as u32;
            let (id_ref, cast_id_ref) = read_context_var_args(player, var_type, ctx.scope_ref());
            let value_ref = {
                let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                scope
                    .stack
                    .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                    .unwrap()
            };

            match put_type {
                PutType::Into => player_set_context_var(
                    player,
                    &id_ref,
                    cast_id_ref.as_ref(),
                    var_type,
                    &value_ref,
                    put_type,
                    &ctx,
                    symbols,
                )?,
                PutType::Before => {
                    let curr_string_id = player_get_context_var(
                        player,
                        &id_ref,
                        cast_id_ref.as_ref(),
                        var_type,
                        &ctx,
                        symbols,
                    )?;
                    let curr_string = player.get_datum(&curr_string_id);
                    let curr_string = curr_string.string_value(symbols)?;
                    let value = player.get_datum(&value_ref);

                    let mut new_string = String::new();
                    new_string.push_str(value.string_value(symbols)?.as_str());
                    new_string.push_str(curr_string.as_str());
                    let new_string = player.alloc_datum(Datum::String(new_string));
                    // Already built the complete string, use Into to replace.
                    player_set_context_var(
                        player,
                        &id_ref,
                        cast_id_ref.as_ref(),
                        var_type,
                        &new_string,
                        PutType::Into,
                        &ctx,
                        symbols,
                    )?;
                }
                PutType::After => {
                    let curr_string_id = player_get_context_var(
                        player,
                        &id_ref,
                        cast_id_ref.as_ref(),
                        var_type,
                        &ctx,
                        symbols,
                    )?;
                    let curr_string = player.get_datum(&curr_string_id);
                    let curr_string = curr_string.string_value(symbols)?;
                    let value = player.get_datum(&value_ref);

                    let mut new_string = String::new();
                    new_string.push_str(curr_string.as_str());
                    new_string.push_str(value.string_value(symbols)?.as_str());
                    let new_string = player.alloc_datum(Datum::String(new_string));
                    // Already built the complete string, use Into to replace.
                    player_set_context_var(
                        player,
                        &id_ref,
                        cast_id_ref.as_ref(),
                        var_type,
                        &new_string,
                        PutType::Into,
                        &ctx,
                        symbols,
                    )?;
                }
            }

            Ok(HandlerExecutionResult::Advance)
        })
    }

    /// Read a chunk operand off the stack as an int without touching the arena.
    /// Only a non-inline entry (a real `DatumRef`) has to be materialized.
    #[inline]
    fn stack_int(
        player: &mut DirPlayer,
        sd: Option<crate::player::scope::StackDatum>,
    ) -> Result<i32, ScriptError> {
        use crate::player::scope::StackDatum;
        match sd {
            Some(StackDatum::Int(n)) => Ok(n),
            // Underflow was previously an `unwrap()` panic; a missing operand
            // means "chunk type not used", which is what 0 encodes.
            Some(StackDatum::Void) | None => Ok(0),
            Some(StackDatum::Float(f)) => Ok(f as i32),
            Some(other) => {
                let value_ref =
                    other.into_ref_with(&mut player.allocator, &mut player.bitmap_manager);
                player.get_datum(&value_ref).int_value()
            }
        }
    }

    fn read_single_chunk_ref(
        player: &mut DirPlayer,
        ctx: &BytecodeHandlerContext,
    ) -> Result<StringChunkExpr, ScriptError> {
        // Pop the eight chunk operands WITHOUT materializing them. `pop_value`
        // keeps inline integers out of the arena; an explicit `into_ref_with`
        // is reserved for consumers that need a `DatumRef`, avoiding the
        // allocations, arena lookups, and frees that materialization would add
        // ~30 ns/op under WASM (vs ~5 ns native — the worst tax on the board).
        // Every one of them is a `pushzero`/`pushint8` already sitting inline on
        // the stack. Same inline-aware pattern `jmp_if_zero` uses.
        let (
            last_line,
            first_line,
            last_item,
            first_item,
            last_word,
            first_word,
            last_char,
            first_char,
        ) = {
            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            let last_line = scope.stack.pop_value();
            let first_line = scope.stack.pop_value();
            let last_item = scope.stack.pop_value();
            let first_item = scope.stack.pop_value();
            let last_word = scope.stack.pop_value();
            let first_word = scope.stack.pop_value();
            let last_char = scope.stack.pop_value();
            let first_char = scope.stack.pop_value();
            (
                last_line, first_line, last_item, first_item, last_word, first_word, last_char,
                first_char,
            )
        };

        let last_line = Self::stack_int(player, last_line)?;
        let first_line = Self::stack_int(player, first_line)?;
        let last_item = Self::stack_int(player, last_item)?;
        let first_item = Self::stack_int(player, first_item)?;
        let last_word = Self::stack_int(player, last_word)?;
        let first_word = Self::stack_int(player, first_word)?;
        let last_char = Self::stack_int(player, last_char)?;
        let first_char = Self::stack_int(player, first_char)?;

        if first_line != 0 || last_line != 0 {
            Ok(StringChunkExpr {
                chunk_type: StringChunkType::Line,
                start: first_line,
                end: last_line,
                item_delimiter: player.movie.item_delimiter.to_owned(),
            })
        } else if first_item != 0 || last_item != 0 {
            Ok(StringChunkExpr {
                chunk_type: StringChunkType::Item,
                start: first_item,
                end: last_item,
                item_delimiter: player.movie.item_delimiter.to_owned(),
            })
        } else if first_word != 0 || last_word != 0 {
            Ok(StringChunkExpr {
                chunk_type: StringChunkType::Word,
                start: first_word,
                end: last_word,
                item_delimiter: player.movie.item_delimiter.to_owned(),
            })
        } else if first_char != 0 || last_char != 0 {
            Ok(StringChunkExpr {
                chunk_type: StringChunkType::Char,
                start: first_char,
                end: last_char,
                item_delimiter: player.movie.item_delimiter.to_owned(),
            })
        } else {
            Err(ScriptError::new(
                "getChunk: invalid chunk range".to_string(),
            ))
        }
    }

    fn read_all_chunks(
        player: &mut DirPlayer,
        ctx: &BytecodeHandlerContext,
    ) -> Result<Vec<StringChunkExpr>, ScriptError> {
        // Pop the eight chunk operands WITHOUT materializing them. `pop_value`
        // keeps inline integers out of the arena; an explicit `into_ref_with`
        // is reserved for consumers that need a `DatumRef`, avoiding the
        // allocations, arena lookups, and frees that materialization would add
        // ~30 ns/op under WASM (vs ~5 ns native — the worst tax on the board).
        // Every one of them is a `pushzero`/`pushint8` already sitting inline on
        // the stack. Same inline-aware pattern `jmp_if_zero` uses.
        let (
            last_line,
            first_line,
            last_item,
            first_item,
            last_word,
            first_word,
            last_char,
            first_char,
        ) = {
            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            let last_line = scope.stack.pop_value();
            let first_line = scope.stack.pop_value();
            let last_item = scope.stack.pop_value();
            let first_item = scope.stack.pop_value();
            let last_word = scope.stack.pop_value();
            let first_word = scope.stack.pop_value();
            let last_char = scope.stack.pop_value();
            let first_char = scope.stack.pop_value();
            (
                last_line, first_line, last_item, first_item, last_word, first_word, last_char,
                first_char,
            )
        };

        let last_line = Self::stack_int(player, last_line)?;
        let first_line = Self::stack_int(player, first_line)?;
        let last_item = Self::stack_int(player, last_item)?;
        let first_item = Self::stack_int(player, first_item)?;
        let last_word = Self::stack_int(player, last_word)?;
        let first_word = Self::stack_int(player, first_word)?;
        let last_char = Self::stack_int(player, last_char)?;
        let first_char = Self::stack_int(player, first_char)?;

        let mut chunks = Vec::new();

        // Add chunks in the order they should be applied
        if first_line != 0 || last_line != 0 {
            chunks.push(StringChunkExpr {
                chunk_type: StringChunkType::Line,
                start: first_line,
                end: last_line,
                item_delimiter: player.movie.item_delimiter.to_owned(),
            });
        }
        if first_item != 0 || last_item != 0 {
            chunks.push(StringChunkExpr {
                chunk_type: StringChunkType::Item,
                start: first_item,
                end: last_item,
                item_delimiter: player.movie.item_delimiter.to_owned(),
            });
        }
        if first_word != 0 || last_word != 0 {
            chunks.push(StringChunkExpr {
                chunk_type: StringChunkType::Word,
                start: first_word,
                end: last_word,
                item_delimiter: player.movie.item_delimiter.to_owned(),
            });
        }
        if first_char != 0 || last_char != 0 {
            chunks.push(StringChunkExpr {
                chunk_type: StringChunkType::Char,
                start: first_char,
                end: last_char,
                item_delimiter: player.movie.item_delimiter.to_owned(),
            });
        }

        if chunks.is_empty() {
            return Err(ScriptError::new(
                "getChunk: no valid chunks specified".to_string(),
            ));
        }

        Ok(chunks)
    }

    pub fn get_chunk(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let string_ref = {
                let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                scope
                    .stack
                    .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                    .unwrap()
            };

            // Read all chunk parameters
            let chunks = Self::read_all_chunks(player, ctx)?;

            // For chunked access on a CastMember (`member.line[5].word[1]`)
            // or on another StringChunk (already nested), build a StringChunk
            // chain rather than flattening to a String. The chain lets
            // downstream property reads — `.fontStyle`, `.font`, `.color`,
            // `.charSpacing` — walk back to the source member and look up
            // STXT formatting_runs / styled spans for the resolved char
            // range. Without this, `member.line[5].word[1].fontStyle`
            // errored with "Invalid string built-in property fontStyle"
            // because the chunk had already lost its member back-reference.
            //
            // Plain `String` inputs (`"hello".word[2]`) have no member to
            // chain to, so the original flatten-to-String behaviour is
            // preserved for them.
            // Inspect the source by REFERENCE. This used to `.clone()` the whole
            // Datum — a full String copy on every chunk read — purely to check
            // which variant it was.
            let initial_chain_source: Option<StringChunkSource> =
                match player.get_datum(&string_ref) {
                    Datum::CastMember(member_ref) => {
                        Some(StringChunkSource::Member(member_ref.clone()))
                    }
                    Datum::StringChunk(..) => Some(StringChunkSource::Datum(string_ref.clone())),
                    _ => None,
                };

            let result_ref = if let Some(initial_source) = initial_chain_source {
                let mut current_source = initial_source;
                let mut current_str = player.get_datum(&string_ref).string_value(symbols)?;
                let mut current_ref = string_ref.clone();
                for expr in chunks.iter() {
                    current_str = StringChunkUtils::resolve_chunk_expr_string(&current_str, expr)?;
                    current_ref = player.alloc_datum(Datum::StringChunk(
                        current_source.clone(),
                        expr.clone(),
                        current_str.clone(),
                    ));
                    current_source = StringChunkSource::Datum(current_ref.clone());
                }
                current_ref
            } else {
                let mut result = player.get_datum(&string_ref).string_value(symbols)?;
                for chunk_expr in chunks {
                    result = StringChunkUtils::resolve_chunk_expr_string(&result, &chunk_expr)?;
                }
                player.alloc_datum(Datum::String(result))
            };

            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            scope.stack.push(result_ref);
            Ok(HandlerExecutionResult::Advance)
        })
    }

    pub fn delete_chunk(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let bytecode_obj = player.get_ctx_current_bytecode(ctx).obj;
            let var_type = bytecode_obj as u32;
            let (id_ref, cast_id_ref) = read_context_var_args(player, var_type, ctx.scope_ref());
            let string_ref = player_get_context_var(
                player,
                &id_ref,
                cast_id_ref.as_ref(),
                var_type,
                ctx,
                symbols,
            )?;
            let chunk_expr = Self::read_single_chunk_ref(player, ctx)?;

            // Strings are value types in Director: `delete char 1 to n of tStr`
            // ASSIGNS a new value to the variable, it does not edit a shared
            // string object. `StringChunkUtils::delete` does `*s = new_string`
            // into the datum behind `string_ref`, which is shared with everyone
            // else holding that same DatumRef — including a CALLER that passed
            // the string as an argument, since parameters are bound to the
            // caller's ref. So a handler that deleted from its own string
            // parameter destroyed the caller's variable.
            //
            // Habbo V31's `explode()` consumes its parameter while parsing
            // (`delete char 1 to tPos + tDelimLength - 1 of tStr` each round), so
            // one call left the caller's variable as just the final field. That
            // broke `rotate`: it explodes the same `tName` once per direction it
            // tries, and from the second iteration on it was exploding "0",
            // hitting `count < 2`, and giving up with "Direction for object not
            // found". Chairs were unaffected only because their cast ships a
            // real direction-4 asset, so they exit on iteration 1.
            //
            // Build the new string and assign it, exactly as `put_chunk` in this
            // file already does.
            //
            // Chunk expressions operate on the value's STRING FORM, so a
            // non-string variable is coerced the same way `&` concatenation
            // coerces it (list literal form). Miniclip's gameloader relies on
            // this: `makeMcExtraJson` does `tx = value(mcExtra)` (a PROP LIST,
            // returned unchanged per the value() contract) and then
            // `delete char 1 of tx` to strip the "[" from its literal form —
            // erroring here aborted the whole wrapper handoff handler.
            let current_datum = player.get_datum(&string_ref);
            let current = match current_datum.string_value(symbols) {
                Ok(s) => s,
                Err(_) => crate::player::datum_formatting::datum_to_string_for_concat(
                    current_datum,
                    symbols,
                    player,
                )?,
            };
            let new_string = StringChunkUtils::string_by_deleting_chunk(&current, &chunk_expr)?;
            let new_string_ref = player.alloc_datum(Datum::String(new_string));
            player_set_context_var(
                player,
                &id_ref,
                cast_id_ref.as_ref(),
                var_type,
                &new_string_ref,
                PutType::Into,
                ctx,
                symbols,
            )?;

            Ok(HandlerExecutionResult::Advance)
        })
    }

    pub fn contains_0str(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let (search_str_ref, search_in_ref) = {
                let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                let search_str_ref = scope
                    .stack
                    .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                    .unwrap();
                let search_in_ref = scope
                    .stack
                    .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                    .unwrap();
                (search_str_ref, search_in_ref)
            };
            let search_in = player.get_datum(&search_in_ref);
            let result = if search_in.is_void() {
                false
            } else {
                // Director's `starts` operator is case-insensitive
                let search_str = player.get_datum(&search_str_ref).string_value(symbols)?;
                let search_in = search_in.string_value(symbols)?;
                search_in
                    .to_ascii_lowercase()
                    .starts_with(search_str.to_ascii_lowercase().as_str())
            };
            let result = player.alloc_datum(datum_bool(result));
            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            scope.stack.push(result);
            Ok(HandlerExecutionResult::Advance)
        })
    }

    pub fn put_chunk(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let bytecode = player.get_ctx_current_bytecode(ctx);
            let put_type = PutType::from(((bytecode.obj >> 4) & 0xF) as u8);
            let var_type = (bytecode.obj & 0xF) as u32;

            // Read the target variable (top of stack: cast_id if field type, then id)
            let (id_ref, cast_id_ref) = read_context_var_args(player, var_type, ctx.scope_ref());

            // Read the chunk expression from the stack (8 values: last_line..first_char)
            let chunk_expr = Self::read_single_chunk_ref(player, ctx)?;

            // Pop the value to put from the stack (pushed before chunk params)
            let value_ref = {
                let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                scope
                    .stack
                    .pop_ref_with(&mut player.allocator, &mut player.bitmap_manager)
                    .unwrap()
            };

            // Field lookups historically swallow ordinary missing-member
            // errors. Validate VM-owned symbols before entering that boundary
            // so a foreign identifier/value cannot be mistaken for a benign
            // field miss and silently discarded.
            if var_type == 0x6 {
                crate::player::context_vars::validate_direct_identifier(
                    player.get_datum(&id_ref),
                    symbols,
                )?;
                if let Some(cast_id_ref) = cast_id_ref.as_ref() {
                    crate::player::context_vars::validate_direct_identifier(
                        player.get_datum(cast_id_ref),
                        symbols,
                    )?;
                }
                crate::player::context_vars::validate_direct_identifier(
                    player.get_datum(&value_ref),
                    symbols,
                )?;
            }

            // Get the current value of the variable
            let string_ref = player_get_context_var(
                player,
                &id_ref,
                cast_id_ref.as_ref(),
                var_type,
                ctx,
                symbols,
            );

            // For field variables (var_type 0x6), Director silently ignores errors
            // (e.g. when a frame script accesses sprite(0).member which is invalid).
            let string_ref = match string_ref {
                Ok(r) => r,
                Err(err) => {
                    if var_type == 0x6 {
                        web_sys::console::warn_1(
                            &format!("putchunk: field lookup failed (ignored): {}", err.message)
                                .into(),
                        );
                        return Ok(HandlerExecutionResult::Advance);
                    }
                    return Err(err);
                }
            };

            let current_string = player.get_datum(&string_ref).string_value(symbols)?;
            let value_string = player.get_datum(&value_ref).string_value(symbols)?;

            // Apply the chunk operation based on put type
            let new_string = match put_type {
                PutType::Into => StringChunkUtils::string_by_putting_into_chunk(
                    &current_string,
                    &chunk_expr,
                    &value_string,
                )?,
                PutType::Before => StringChunkUtils::string_by_putting_before_chunk(
                    &current_string,
                    &chunk_expr,
                    &value_string,
                )?,
                PutType::After => StringChunkUtils::string_by_putting_after_chunk(
                    &current_string,
                    &chunk_expr,
                    &value_string,
                )?,
            };

            let new_string_ref = player.alloc_datum(Datum::String(new_string));
            // The chunk operation already built the complete result string,
            // so always use Into to replace rather than append/prepend again.
            let set_result = player_set_context_var(
                player,
                &id_ref,
                cast_id_ref.as_ref(),
                var_type,
                &new_string_ref,
                PutType::Into,
                ctx,
                symbols,
            );
            // For field variables, silently ignore set failures too
            if let Err(err) = set_result {
                if var_type == 0x6 {
                    web_sys::console::warn_1(
                        &format!("putchunk: field set failed (ignored): {}", err.message).into(),
                    );
                    return Ok(HandlerExecutionResult::Advance);
                }
                return Err(err);
            }

            Ok(HandlerExecutionResult::Advance)
        })
    }
}
