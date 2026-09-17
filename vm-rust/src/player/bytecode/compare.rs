use num::ToPrimitive;

use crate::{
    director::lingo::datum::{datum_bool, Datum},
    player::{
        compare::{datum_equals, datum_greater_than, datum_less_than},
        reserve_player_mut, scope::StackDatum, HandlerExecutionResult, ScriptError,
        symbols::symbol_table::SymbolTable,
    },
};

use super::handler_manager::BytecodeHandlerContext;

pub struct CompareBytecodeHandler {}

#[inline]
fn validate_inline_symbol(
    symbol: &crate::player::symbols::symbol::Symbol,
    symbols: &SymbolTable,
) -> Result<(), ScriptError> {
    symbols
        .display(symbol)
        .map(|_| ())
        .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign.into())
}

impl CompareBytecodeHandler {
    pub fn gt(runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let (lv, rv) = Self::pop2(player, ctx, "gt")?;
            if let (StackDatum::Int(a), StackDatum::Int(b)) = (&lv, &rv) {
                let r = (*a > *b) as i32;
                player.scopes.get_mut(ctx.scope_ref()).unwrap().stack.push_int(r);
                return Ok(HandlerExecutionResult::Advance);
            }
            let right = rv.into_ref_with(&mut player.allocator, &mut player.bitmap_manager);
            let left = lv.into_ref_with(&mut player.allocator, &mut player.bitmap_manager);
            let is_gt = datum_greater_than(player.get_datum(&left), player.get_datum(&right), &player.allocator, symbols)?;
            let result_id = player.alloc_datum(datum_bool(is_gt));
            player.scopes.get_mut(ctx.scope_ref()).unwrap().stack.push(result_id);
            Ok(HandlerExecutionResult::Advance)
        })
    }

    pub fn lt(runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let (lv, rv) = Self::pop2(player, ctx, "lt")?;
            if let (StackDatum::Int(a), StackDatum::Int(b)) = (&lv, &rv) {
                let r = (*a < *b) as i32;
                player.scopes.get_mut(ctx.scope_ref()).unwrap().stack.push_int(r);
                return Ok(HandlerExecutionResult::Advance);
            }
            let right = rv.into_ref_with(&mut player.allocator, &mut player.bitmap_manager);
            let left = lv.into_ref_with(&mut player.allocator, &mut player.bitmap_manager);
            let is_lt = datum_less_than(player.get_datum(&left), player.get_datum(&right), &player.allocator, symbols)?;
            let result_id = player.alloc_datum(datum_bool(is_lt));
            player.scopes.get_mut(ctx.scope_ref()).unwrap().stack.push(result_id);
            Ok(HandlerExecutionResult::Advance)
        })
    }

    pub fn lt_eq(runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let (lv, rv) = Self::pop2(player, ctx, "lt_eq")?;
            if let (StackDatum::Int(a), StackDatum::Int(b)) = (&lv, &rv) {
                let r = (*a <= *b) as i32;
                player.scopes.get_mut(ctx.scope_ref()).unwrap().stack.push_int(r);
                return Ok(HandlerExecutionResult::Advance);
            }
            let right = rv.into_ref_with(&mut player.allocator, &mut player.bitmap_manager);
            let left = lv.into_ref_with(&mut player.allocator, &mut player.bitmap_manager);
            let l = player.get_datum(&left);
            let rgt = player.get_datum(&right);
            let is_lt = datum_less_than(l, rgt, &player.allocator, symbols)?;
            let is_eq = datum_equals(l, rgt, &player.allocator, symbols)?;
            let result_id = player.alloc_datum(datum_bool(is_lt || is_eq));
            player.scopes.get_mut(ctx.scope_ref()).unwrap().stack.push(result_id);
            Ok(HandlerExecutionResult::Advance)
        })
    }

    pub fn gt_eq(runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let (lv, rv) = Self::pop2(player, ctx, "gt_eq")?;
            if let (StackDatum::Int(a), StackDatum::Int(b)) = (&lv, &rv) {
                let r = (*a >= *b) as i32;
                player.scopes.get_mut(ctx.scope_ref()).unwrap().stack.push_int(r);
                return Ok(HandlerExecutionResult::Advance);
            }
            let right = rv.into_ref_with(&mut player.allocator, &mut player.bitmap_manager);
            let left = lv.into_ref_with(&mut player.allocator, &mut player.bitmap_manager);
            let l = player.get_datum(&left);
            let rgt = player.get_datum(&right);
            let is_gt = datum_greater_than(l, rgt, &player.allocator, symbols)?;
            let is_eq = datum_equals(l, rgt, &player.allocator, symbols)?;
            let result_id = player.alloc_datum(datum_bool(is_gt || is_eq));
            player.scopes.get_mut(ctx.scope_ref()).unwrap().stack.push(result_id);
            Ok(HandlerExecutionResult::Advance)
        })
    }

    /// Pop two operands as raw `StackDatum`s (left, right) without
    /// materializing — the int/int fast path in each comparison reads them
    /// directly. `op` names the opcode for the underflow message.
    #[inline]
    fn pop2(
        player: &mut crate::player::DirPlayer,
        ctx: &BytecodeHandlerContext,
        op: &str,
    ) -> Result<(StackDatum, StackDatum), ScriptError> {
        let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
        let right = scope
            .stack
            .pop_value()
            .ok_or_else(|| ScriptError::new(format!("{}: stack underflow (right)", op)))?;
        let left = scope
            .stack
            .pop_value()
            .ok_or_else(|| ScriptError::new(format!("{}: stack underflow (left)", op)))?;
        Ok((left, right))
    }

    pub fn not(runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player(|player| {
            let obj_id = {
                let (scopes, allocator, bitmap_manager) =
                    (&mut player.scopes, &mut player.allocator, &mut player.bitmap_manager);
                let scope = scopes.get_mut(ctx.scope_ref()).unwrap();
                scope.stack.pop_ref_with(allocator, bitmap_manager)
                    .ok_or_else(|| ScriptError::new("not: stack underflow".to_string()))?
            };
            let obj = player.get_datum(&obj_id);
            let is_not = match obj {
                Datum::Void => true,
                Datum::Int(num) => *num == 0,
                Datum::Float(num) => num.to_u64().unwrap() == 0,
                _ => false,
            };
            let result_id = player.alloc_datum(datum_bool(is_not));
            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            scope.stack.push(result_id);
            Ok(HandlerExecutionResult::Advance)
        })
    }

    pub fn nt_eq(runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let (lv, rv) = Self::pop2(player, ctx, "nt_eq")?;
            if let (StackDatum::Int(a), StackDatum::Int(b)) = (&lv, &rv) {
                let r = (*a != *b) as i32;
                player.scopes.get_mut(ctx.scope_ref()).unwrap().stack.push_int(r);
                return Ok(HandlerExecutionResult::Advance);
            }
            // See `eq`: same spur compare, negated.
            if let (StackDatum::Symbol(a), StackDatum::Symbol(b)) = (&lv, &rv) {
                validate_inline_symbol(a, symbols)?;
                validate_inline_symbol(b, symbols)?;
                let r = (a != b) as i32;
                player.scopes.get_mut(ctx.scope_ref()).unwrap().stack.push_int(r);
                return Ok(HandlerExecutionResult::Advance);
            }
            let right = rv.into_ref_with(&mut player.allocator, &mut player.bitmap_manager);
            let left = lv.into_ref_with(&mut player.allocator, &mut player.bitmap_manager);
            let is_eq = datum_equals(player.get_datum(&left), player.get_datum(&right), &player.allocator, symbols)?;
            let result_id = player.alloc_datum(datum_bool(!is_eq));
            player.scopes.get_mut(ctx.scope_ref()).unwrap().stack.push(result_id);
            Ok(HandlerExecutionResult::Advance)
        })
    }

    pub fn and(runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player(|player| {
            let (left, right) = {
                let (scopes, allocator, bitmap_manager) =
                    (&mut player.scopes, &mut player.allocator, &mut player.bitmap_manager);
                let scope = scopes.get_mut(ctx.scope_ref()).unwrap();
                let right = scope.stack.pop_ref_with(allocator, bitmap_manager)
                    .ok_or_else(|| ScriptError::new("and: stack underflow (right)".to_string()))?;
                let left = scope.stack.pop_ref_with(allocator, bitmap_manager)
                    .ok_or_else(|| ScriptError::new("and: stack underflow (left)".to_string()))?;
                (left, right)
            };
            let right = player.get_datum(&right);
            let left = player.get_datum(&left);

            let is_and = left.to_bool()? && right.to_bool()?;

            let result_id = player.alloc_datum(datum_bool(is_and));

            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            scope.stack.push(result_id);
            Ok(HandlerExecutionResult::Advance)
        })
    }

    pub fn or(runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player(|player| {
            let (left, right) = {
                let (scopes, allocator, bitmap_manager) =
                    (&mut player.scopes, &mut player.allocator, &mut player.bitmap_manager);
                let scope = scopes.get_mut(ctx.scope_ref()).unwrap();
                let right = scope.stack.pop_ref_with(allocator, bitmap_manager)
                    .ok_or_else(|| ScriptError::new("or: stack underflow (right)".to_string()))?;
                let left = scope.stack.pop_ref_with(allocator, bitmap_manager)
                    .ok_or_else(|| ScriptError::new("or: stack underflow (left)".to_string()))?;
                (left, right)
            };
            let right = player.get_datum(&right);
            let left = player.get_datum(&left);

            let is_or = left.to_bool()? || right.to_bool()?;

            let result_id = player.alloc_datum(datum_bool(is_or));
            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            scope.stack.push(result_id);
            Ok(HandlerExecutionResult::Advance)
        })
    }

    pub fn eq(runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext) -> Result<HandlerExecutionResult, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let (lv, rv) = Self::pop2(player, ctx, "eq")?;
            if let (StackDatum::Int(a), StackDatum::Int(b)) = (&lv, &rv) {
                let r = (*a == *b) as i32;
                player.scopes.get_mut(ctx.scope_ref()).unwrap().stack.push_int(r);
                return Ok(HandlerExecutionResult::Advance);
            }
            // `#symbol = #symbol` is idiomatic Lingo and was taking the slow
            // path: two `into_ref` (symbol-pool lookups), two arena `get_datum`,
            // `datum_equals`, and an `alloc_datum` for the boolean result.
            // `datum_equals` resolves this arm as `s == other` — a spur compare —
            // so this is the same answer without touching the arena. Symbol
            // identity is case-insensitive because `intern` lowercases, which is
            // Director's rule.
            if let (StackDatum::Symbol(a), StackDatum::Symbol(b)) = (&lv, &rv) {
                validate_inline_symbol(a, symbols)?;
                validate_inline_symbol(b, symbols)?;
                let r = (a == b) as i32;
                player.scopes.get_mut(ctx.scope_ref()).unwrap().stack.push_int(r);
                return Ok(HandlerExecutionResult::Advance);
            }
            let right = rv.into_ref_with(&mut player.allocator, &mut player.bitmap_manager);
            let left = lv.into_ref_with(&mut player.allocator, &mut player.bitmap_manager);
            let is_eq = datum_equals(player.get_datum(&left), player.get_datum(&right), &player.allocator, symbols)?;
            let result_id = player.alloc_datum(datum_bool(is_eq));
            player.scopes.get_mut(ctx.scope_ref()).unwrap().stack.push(result_id);
            Ok(HandlerExecutionResult::Advance)
        })
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::HashMap, rc::Rc};

    use crate::{
        director::{chunks::{handler::HandlerDef, script::ScriptChunk}, enums::ScriptType},
        player::{
            cast_lib::CastMemberRef,
            ownership::{OwnerKey, OwnerToken},
            scope::ScopeRef,
            script::Script,
            session::ExecutionContext,
            symbols::symbol::Symbol,
            ScopeToken, DirPlayer,
        },
    };
    use crate::player::bytecode::handler_manager::HandlerCode;

    fn make_player() -> DirPlayer {
        let (tx, _rx) = async_std::channel::unbounded();
        DirPlayer::new_with_owner(
            tx,
            OwnerToken::new(OwnerKey { session: 17, player: 4, generation: 1 }),
        )
    }

    fn make_context(player: &DirPlayer, slot: ScopeRef) -> BytecodeHandlerContext {
        let handler = Rc::new(HandlerDef {
            name_id: 0,
            bytecode_array: vec![],
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
    fn symbol_fast_paths_validate_owner_before_identity_compare() {
        let mut player = make_player();
        let mut symbols = SymbolTable::new();
        let local = symbols.intern("localSymbol");
        let slot = player.push_scope();
        player.scopes[slot].stack.push_value(StackDatum::Symbol(local.clone()));
        player.scopes[slot].stack.push_value(StackDatum::Symbol(local));
        let ctx = make_context(&player, slot);
        let datum_count_before = player.allocator.datum_count();
        let mut runtime = ExecutionContext { player_id: 4, symbols: &mut symbols, player: &mut player };
        assert!(matches!(CompareBytecodeHandler::eq(&mut runtime, &ctx), Ok(HandlerExecutionResult::Advance)));
        assert_eq!(player.allocator.datum_count(), datum_count_before);
        assert!(matches!(player.scopes[slot].stack.pop_value(), Some(StackDatum::Int(1))));
        player.pop_scope();

        let left = symbols.intern("localLeft");
        let right = symbols.intern("localRight");
        let slot = player.push_scope();
        player.scopes[slot].stack.push_value(StackDatum::Symbol(left));
        player.scopes[slot].stack.push_value(StackDatum::Symbol(right));
        let ctx = make_context(&player, slot);
        let mut runtime = ExecutionContext { player_id: 4, symbols: &mut symbols, player: &mut player };
        assert!(matches!(CompareBytecodeHandler::nt_eq(&mut runtime, &ctx), Ok(HandlerExecutionResult::Advance)));
        assert!(matches!(player.scopes[slot].stack.pop_value(), Some(StackDatum::Int(1))));
        player.pop_scope();

        let mut foreign_table = SymbolTable::new();
        let foreign = foreign_table.intern("foreignSymbol");
        let slot = player.push_scope();
        player.scopes[slot].stack.push_value(StackDatum::Symbol(foreign.clone()));
        player.scopes[slot].stack.push_value(StackDatum::Symbol(foreign));
        let ctx = make_context(&player, slot);
        let mut runtime = ExecutionContext { player_id: 4, symbols: &mut symbols, player: &mut player };
        assert!(CompareBytecodeHandler::eq(&mut runtime, &ctx).is_err());
        player.pop_scope();
    }

    #[test]
    fn symbol_not_equal_fast_path_rejects_two_foreign_symbols() {
        let mut player = make_player();
        let mut symbols = SymbolTable::new();
        let mut foreign_table = SymbolTable::new();
        let foreign = foreign_table.intern("foreignNotEqual");
        let slot = player.push_scope();
        player.scopes[slot].stack.push_value(StackDatum::Symbol(foreign.clone()));
        player.scopes[slot].stack.push_value(StackDatum::Symbol(foreign));
        let ctx = make_context(&player, slot);
        let mut runtime = ExecutionContext { player_id: 4, symbols: &mut symbols, player: &mut player };
        assert!(CompareBytecodeHandler::nt_eq(&mut runtime, &ctx).is_err());
        player.pop_scope();
    }
}
