use std::{collections::VecDeque, hash::{Hash, Hasher}, rc::Rc};
use log::error;
use pest::{
    iterators::{Pair, Pairs},
    pratt_parser::{Assoc, Op, PrattParser},
    Parser,
};

use crate::{
    director::lingo::datum::{Datum, DatumType, StringChunkExpr, StringChunkType, datum_bool},
    js_api::ascii_safe,
    player::{
        DirPlayer, bytecode::{get_set::GetSetUtils, string::StringBytecodeHandler}, datum_operations::{add_datums, divide_datums, multiply_datums, subtract_datums}, handlers::datum_handlers::{prop_list::PropListUtils, string_chunk::StringChunkUtils}, script::{get_lctx_for_script, get_obj_prop, script_get_prop_opt, script_set_prop}, symbols::{builtin::BuiltInSymbol, symbol::Symbol, symbol_table::SymbolTable}
    },
};

use super::{cast_lib::INVALID_CAST_MEMBER_REF, sprite::ColorRef, DatumRef, ScriptError};

#[derive(Parser)]
#[grammar = "lingo.pest"]
struct LingoParser;

fn tokenize_lingo(_expr: &str) -> Vec<String> {
    [].to_vec()
}

/// Opaque identity for one session-owned evaluator. The serial is only a
/// lookup hint; accepting a completion also requires this exact capability
/// allocation, so a fabricated numeric value cannot target a live evaluator.
#[derive(Debug)]
struct EvalCapability;

#[derive(Clone, Debug)]
pub(crate) struct EvalId {
    serial: u64,
    capability: Rc<EvalCapability>,
}

impl EvalId {
    pub(crate) fn new(serial: u64) -> Self {
        Self { serial, capability: Rc::new(EvalCapability) }
    }
}

impl PartialEq for EvalId {
    fn eq(&self, other: &Self) -> bool {
        self.serial == other.serial && Rc::ptr_eq(&self.capability, &other.capability)
    }
}

impl Eq for EvalId {}

impl Hash for EvalId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.serial.hash(state);
        (Rc::as_ptr(&self.capability) as usize).hash(state);
    }
}

/// External work requested by an evaluator turn. Requests own every VM handle
/// and are consumed by the session/driver owner after the borrow ends.
pub(crate) enum EvalPending {
    Global {
        capability: EvalAction,
        request: crate::player::driver::InternalVmRequest,
        reason: Option<String>,
        prepared_child: Option<PreparedGlobal>,
    },
    Object {
        capability: EvalAction,
        request: crate::player::driver::InternalVmRequest,
        reason: Option<String>,
    },
    SetProperty { capability: EvalAction, request: crate::player::driver::InternalVmRequest },
}

pub(crate) enum PreparedGlobal {
    Child {
        receiver: Option<crate::player::ScriptInstanceRef>,
        handler_ref: crate::player::script::ScriptHandlerRef,
        args: Vec<crate::player::DatumRef>,
        use_raw_arg_list: bool,
        completion: Option<crate::player::driver::ChildCompletion>,
    },
    Ancestors {
        calls: Vec<crate::player::handlers::types::AncestorCall>,
    },
    Broadcast {
        plan: crate::player::driver::BroadcastPlan,
    },
}

#[derive(Debug)]
struct EvalActionCapability;

#[derive(Clone, Debug)]
pub(crate) struct EvalAction {
    serial: u64,
    capability: Rc<EvalActionCapability>,
}

impl EvalAction {
    fn new(serial: u64) -> Self {
        Self { serial, capability: Rc::new(EvalActionCapability) }
    }
}

impl PartialEq for EvalAction {
    fn eq(&self, other: &Self) -> bool {
        self.serial == other.serial && Rc::ptr_eq(&self.capability, &other.capability)
    }
}

impl Eq for EvalAction {}

pub(crate) enum EvalState {
    Ready,
    Running,
    Waiting,
    Completed(Result<DatumRef, ScriptError>),
}

/// Canonical owned evaluator boundary. The expression and owner capability
/// outlive every synchronous turn; no player or symbol-table borrow is stored.
pub(crate) struct EvalContinuation {
    pub(crate) id: EvalId,
    pub(crate) player_id: crate::player::session::PlayerId,
    pub(crate) owner: crate::player::ownership::OwnerToken,
    pub(crate) scope: Option<crate::player::ScopeToken>,
    pub(crate) expression: LingoExpr,
    pub(crate) state: EvalState,
    frames: Vec<EvalFrame>,
    values: Vec<DatumRef>,
    command_lines: Option<Vec<String>>,
    next_command_line: usize,
    next_action: u64,
    waiting_action: Option<EvalAction>,
    waiting_discards_result: bool,
    callback_sender: Option<async_std::channel::Sender<Result<crate::player::scope::ScopeResult, ScriptError>>>,
}

impl EvalContinuation {
    pub(crate) fn new(
        session: &mut crate::player::session::RuntimeSession,
        player_id: crate::player::session::PlayerId,
        id: EvalId,
        expression: LingoExpr,
    ) -> Result<Self, ScriptError> {
        let (owner, scope) = session
            .with_player(player_id, |context| -> Result<_, ScriptError> {
                let scope = if context.player.scope_count > 0 {
                    let candidate = if context.player.current_breakpoint.is_some() {
                        context.player.eval_scope_index
                            .unwrap_or(context.player.scope_count - 1) as usize
                    } else {
                        context.player.scope_count as usize - 1
                    };
                    if candidate >= context.player.scope_count as usize
                        || candidate >= context.player.scopes.len()
                    {
                        return Err(ScriptError::new("debugger evaluator scope index is invalid".to_owned()));
                    }
                    let live_scope = &context.player.scopes[candidate];
                    Some(crate::player::ScopeToken {
                        owner: context.player.owner.clone(),
                        slot: candidate,
                        generation: live_scope.generation,
                        epoch: context.player.scope_invalidation_epoch,
                    })
                } else { None };
                Ok((context.player.owner.clone(), scope))
            })
            .ok_or_else(crate::player::cancelled_scope_error)??;
        Ok(Self {
            id,
            player_id,
            owner,
            scope,
            frames: vec![EvalFrame::Evaluate(expression.clone())],
            expression,
            values: Vec::new(),
            state: EvalState::Ready,
            next_action: 1,
            waiting_action: None,
            waiting_discards_result: false,
            callback_sender: None,
            command_lines: None,
            next_command_line: 0,
        })
    }

    pub(crate) fn accepts_owner(&self, owner: &crate::player::ownership::OwnerToken) -> bool {
        self.owner.same_identity(owner) && self.owner.is_arena_live()
    }

    pub(crate) fn child_anchor(&self) -> (crate::player::ownership::OwnerToken, Option<crate::player::ScopeToken>) {
        (self.owner.clone(), self.scope.clone())
    }

    pub(crate) fn waiting_for(&self, action: &EvalAction) -> bool {
        matches!(self.state, EvalState::Waiting) && self.waiting_action.as_ref() == Some(action)
    }

    pub(crate) fn new_command(
        session: &mut crate::player::session::RuntimeSession,
        player_id: crate::player::session::PlayerId,
        id: EvalId,
        lines: Vec<String>,
    ) -> Result<Self, ScriptError> {
        let mut continuation = Self::new(session, player_id, id, LingoExpr::VoidLiteral)?;
        continuation.frames.clear();
        continuation.command_lines = Some(lines);
        Ok(continuation)
    }

    /// Create a continuation whose root work is already an owned external
    /// request. This is used by event/actor adapters that have no source
    /// expression to evaluate but still need the evaluator's real owner and
    /// capability validation for completion and reset.
    pub(crate) fn new_request(
        session: &mut crate::player::session::RuntimeSession,
        player_id: crate::player::session::PlayerId,
        id: EvalId,
        request: crate::player::driver::InternalVmRequest,
    ) -> Result<(Self, EvalPending), ScriptError> {
        let mut continuation = Self::new(session, player_id, id, LingoExpr::VoidLiteral)?;
        continuation.frames.clear();
        continuation.state = EvalState::Waiting;
        let capability = continuation.new_action(false);
        let pending = match request {
            crate::player::driver::InternalVmRequest::Global { name, args } => EvalPending::Global {
                capability,
                request: crate::player::driver::InternalVmRequest::Global { name, args },
                reason: None,
                prepared_child: None,
            },
            crate::player::driver::InternalVmRequest::Object { receiver, name, args } => EvalPending::Object {
                capability,
                request: crate::player::driver::InternalVmRequest::Object { receiver, name, args },
                reason: None,
            },
            crate::player::driver::InternalVmRequest::Flash(request) => EvalPending::Object {
                capability,
                request: crate::player::driver::InternalVmRequest::Flash(request),
                reason: None,
            },
            crate::player::driver::InternalVmRequest::ObjectProperty { receiver, name } => EvalPending::Object {
                capability,
                request: crate::player::driver::InternalVmRequest::ObjectProperty { receiver, name },
                reason: None,
            },
            crate::player::driver::InternalVmRequest::SetProperty { receiver, name, value } => EvalPending::SetProperty {
                capability,
                request: crate::player::driver::InternalVmRequest::SetProperty { receiver, name, value },
            },
            _ => return Err(ScriptError::new("standalone evaluator request is not a global, object, or property operation".to_owned())),
        };
        Ok((continuation, pending))
    }

    pub(crate) fn set_callback_sender(
        &mut self,
        sender: async_std::channel::Sender<Result<crate::player::scope::ScopeResult, ScriptError>>,
    ) {
        self.callback_sender = Some(sender);
    }

    pub(crate) fn take_callback_sender(
        &mut self,
    ) -> Option<async_std::channel::Sender<Result<crate::player::scope::ScopeResult, ScriptError>>> {
        self.callback_sender.take()
    }

    pub(crate) fn turn(
        &mut self,
        session: &mut crate::player::session::RuntimeSession,
    ) -> EvalTurn {
        let owner_active = session
            .with_player(self.player_id, |context| {
                self.accepts_owner(&context.player.owner)
                    && self.scope.as_ref().map_or(true, |scope| scope.validate_active(context.player))
            })
            .unwrap_or(false);
        if !owner_active {
            let error = crate::player::cancelled_scope_error();
            self.state = EvalState::Completed(Err(error.clone()));
            return EvalTurn::Complete(Err(error));
        }
        if !matches!(self.state, EvalState::Ready) {
            return EvalTurn::Complete(Err(ScriptError::new(
                "evaluator turn is not ready".to_owned(),
            )));
        }
        loop {
            if self.frames.is_empty() {
                // A root expression can complete an external action with no
                // parent frame. Preserve that value before asking the lazy
                // command source for another line; otherwise a resumed root
                // handler incorrectly returns VOID.
                if let Some(value) = self.values.pop() {
                    if self.command_lines.as_ref().is_some_and(|lines| self.next_command_line < lines.len()) {
                        self.values.clear();
                        self.state = EvalState::Ready;
                        continue;
                    }
                    self.state = EvalState::Completed(Ok(value.clone()));
                    return EvalTurn::Complete(Ok(value));
                }
                match self.next_command_ast() {
                    Ok(Some(ast)) => self.frames.push(EvalFrame::Evaluate(ast)),
                    Ok(None) => {
                        self.state = EvalState::Completed(Ok(DatumRef::Void));
                        return EvalTurn::Complete(Ok(DatumRef::Void));
                    }
                    Err(error) => {
                        self.state = EvalState::Completed(Err(error.clone()));
                        return EvalTurn::Complete(Err(error));
                    }
                }
            }
            self.state = EvalState::Running;
            match self.run_frames(session) {
                EvalTurn::Pending { request } => {
                    self.state = EvalState::Waiting;
                    return EvalTurn::Pending { request };
                }
                EvalTurn::Complete(Err(error)) => {
                    self.state = EvalState::Completed(Err(error.clone()));
                    return EvalTurn::Complete(Err(error));
                }
                EvalTurn::Complete(Ok(value)) => {
                    if self.command_lines.as_ref().is_some_and(|lines| self.next_command_line < lines.len()) {
                        self.values.clear();
                        self.state = EvalState::Ready;
                        continue;
                    }
                    self.state = EvalState::Completed(Ok(value.clone()));
                    return EvalTurn::Complete(Ok(value));
                }
            }
        }
    }


    pub(crate) fn complete(
        &mut self,
        session: &mut crate::player::session::RuntimeSession,
        action: &EvalAction,
        owner: &crate::player::ownership::OwnerToken,
        result: Result<DatumRef, ScriptError>,
    ) -> bool {
        if !self.accepts_owner(owner)
            || !matches!(self.state, EvalState::Waiting)
            || self.waiting_action.as_ref() != Some(action)
        {
            return false;
        }
        let current_owner = session
            .with_player(self.player_id, |context| {
                self.accepts_owner(&context.player.owner)
                    && self.scope.as_ref().map_or(true, |scope| scope.validate_active(context.player))
            })
            .unwrap_or(false);
        if !current_owner {
            return false;
        }
        let result_valid = match &result {
            Ok(value) => session
                .with_player(self.player_id, |context| {
                    crate::player::driver::checked_internal_datum(context.player, context.symbols, value)
                        .map(|_| ())
                })
                .and_then(Result::ok)
                .is_some(),
            Err(_) => true,
        };
        if !result_valid {
            return false;
        }
        self.waiting_action = None;
        let value = match result {
            Ok(value) => value,
            Err(error) => {
                self.state = EvalState::Completed(Err(error));
                return true;
            }
        };
        self.values.push(if self.waiting_discards_result { DatumRef::Void } else { value });
        self.waiting_discards_result = false;
        self.state = EvalState::Ready;
        true
    }

    fn new_action(&mut self, discards_result: bool) -> EvalAction {
        let serial = self.next_action;
        self.next_action = self.next_action.wrapping_add(1).max(1);
        let action = EvalAction::new(serial);
        self.waiting_action = Some(action.clone());
        self.waiting_discards_result = discards_result;
        action
    }

    fn next_command_ast(&mut self) -> Result<Option<LingoExpr>, ScriptError> {
        let Some(lines) = self.command_lines.as_ref() else { return Ok(None) };
        let Some(line) = lines.get(self.next_command_line).cloned() else { return Ok(None) };
        self.next_command_line += 1;
        parse_lingo_expr_ast_runtime(Rule::command_eval_expr, line)
            .map(Some)
    }

}

#[derive(Debug, Clone, PartialEq)]
pub enum LingoExpr {
    SymbolLiteral(String),
    StringLiteral(String),
    ListLiteral(Vec<LingoExpr>),
    VoidLiteral,
    BoolLiteral(bool),
    IntLiteral(i32),
    FloatLiteral(f64),
    PropListLiteral(Vec<(LingoExpr, LingoExpr)>),
    HandlerCall(String, Vec<LingoExpr>),
    ObjProp(Box<LingoExpr>, String),
    ObjHandlerCall(Box<LingoExpr>, String, Vec<LingoExpr>),
    ListAccess(Box<LingoExpr>, Box<LingoExpr>), // list_expr, index_expr
    ColorLiteral(ColorRef),
    RectLiteral(Vec<(LingoExpr, LingoExpr, LingoExpr, LingoExpr)>),
    PointLiteral(Vec<(LingoExpr, LingoExpr)>),
    MemberRef(Box<LingoExpr>, Option<Box<LingoExpr>>), // member_num, optional cast_lib
    Identifier(String),
    Assignment(Box<LingoExpr>, Box<LingoExpr>),
    Add(Box<LingoExpr>, Box<LingoExpr>),
    Subtract(Box<LingoExpr>, Box<LingoExpr>),
    Multiply(Box<LingoExpr>, Box<LingoExpr>),
    Divide(Box<LingoExpr>, Box<LingoExpr>),
    Modulo(Box<LingoExpr>, Box<LingoExpr>),
    Join(Box<LingoExpr>, Box<LingoExpr>),
    JoinPad(Box<LingoExpr>, Box<LingoExpr>),
    And(Box<LingoExpr>, Box<LingoExpr>),
    Or(Box<LingoExpr>, Box<LingoExpr>),
    Eq(Box<LingoExpr>, Box<LingoExpr>),
    Ne(Box<LingoExpr>, Box<LingoExpr>),
    Lt(Box<LingoExpr>, Box<LingoExpr>),
    Gt(Box<LingoExpr>, Box<LingoExpr>),
    Le(Box<LingoExpr>, Box<LingoExpr>),
    Ge(Box<LingoExpr>, Box<LingoExpr>),
    Not(Box<LingoExpr>),
    PutBefore(Box<LingoExpr>, Box<LingoExpr>),
    PutAfter(Box<LingoExpr>, Box<LingoExpr>),
    PutInto(Box<LingoExpr>, Box<LingoExpr>),
    PutDisplay(Box<LingoExpr>),
    ThePropOf(Box<LingoExpr>, String), // "the X of Y" constructs
    ChunkExpr(Symbol, Box<LingoExpr>, Option<Box<LingoExpr>>, Box<LingoExpr>),
    DeleteChunk(Box<LingoExpr>), // delete <chunk_expr>
    /// `if EXPR then COMMAND` inline form (no `else`, no `end if`). Used
    /// by Director's text-eval contexts like `keyUpScript`, where the
    /// assigned source is typically a single line: `if the key = "d"
    /// then sendAllSprites(#toggleDebug)`. Multi-line `if … end if`
    /// blocks live in the compiled bytecode path and aren't parsed by
    /// the AST evaluator.
    IfThen(Box<LingoExpr>, Box<LingoExpr>),
    /// `pass` keyword — Lingo's "don't consume this event, let it
    /// propagate". In a script-text context (where no event-dispatch
    /// chain is being walked) it's effectively a no-op; we evaluate to
    /// Void.
    Pass,
}

/// Evaluate a static Lingo expression. This does not support function calls.
fn checked_static_datum(
    player: &DirPlayer,
    datum_ref: &DatumRef,
    symbols: &SymbolTable,
) -> Result<Datum, ScriptError> {
    let datum = match datum_ref {
        DatumRef::Void => Datum::Void,
        DatumRef::Ref(_) => player
            .allocator
            .try_get_datum(datum_ref)
            .cloned()
            .ok_or_else(|| ScriptError::new_code(
                crate::player::ScriptErrorCode::InvalidReference,
                "Foreign or stale datum reference".to_string(),
            ))?,
    };
    crate::player::compare::validate_direct_symbol_fields(&datum, symbols)?;
    Ok(datum)
}

fn validate_static_stack_datum(
    player: &DirPlayer,
    value: &crate::player::scope::StackDatum,
    symbols: &SymbolTable,
) -> Result<(), ScriptError> {
    match value {
        crate::player::scope::StackDatum::Symbol(symbol) => symbols
            .display(symbol)
            .map(|_| ())
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign.into()),
        crate::player::scope::StackDatum::Ref(datum_ref) => {
            checked_static_datum(player, datum_ref, symbols).map(|_| ())
        }
        _ => Ok(()),
    }
}

fn eval_static_term_with_prefix(
    first: Pair<Rule>,
    iter: &mut std::vec::IntoIter<Pair<Rule>>,
    player: &mut DirPlayer,
    symbols: &mut SymbolTable,
) -> Result<DatumRef, ScriptError> {
    match first.as_rule() {
        Rule::not_op => {
            let operand = iter.next().ok_or_else(|| {
                ScriptError::new("Expected operand after not".to_string())
            })?;
            let operand_ref = eval_static_term_with_prefix(operand, iter, player, symbols)?;
            let value = checked_static_datum(player, &operand_ref, symbols)?.bool_value()?;
            Ok(player.alloc_datum(Datum::Int(i32::from(!value))))
        }
        Rule::neg_op => {
            let operand = iter.next().ok_or_else(|| {
                ScriptError::new("Expected operand after unary minus".to_string())
            })?;
            let operand_ref = eval_static_term_with_prefix(operand, iter, player, symbols)?;
            let minus_one = player.alloc_datum(Datum::Int(-1));
            let value = crate::player::datum_operations::multiply_datums(
                operand_ref,
                minus_one,
                player,
                symbols,
            )?;
            Ok(player.alloc_datum(value))
        }
        _ => eval_lingo_pair_static(first, player, symbols),
    }
}

pub fn eval_lingo_pair_static(
    pair: Pair<Rule>,
    player: &mut DirPlayer,
    symbols: &mut SymbolTable,
) -> Result<DatumRef, ScriptError> {
    let inner_rule = pair.as_rule();
    match pair.as_rule() {
        Rule::expr => {
            let mut inner_pairs: Vec<Pair<Rule>> = pair.into_inner().collect();
            if inner_pairs.len() == 1 {
                return eval_lingo_pair_static(inner_pairs.remove(0), player, symbols);
            }
            // Handle binary operators (e.g. "foo" & QUOTE & "bar")
            let mut iter = inner_pairs.into_iter();
            let first = iter.next()
                .ok_or_else(|| ScriptError::new("Expected expression content".to_string()))?;
            // Prefix operators bind to one term. Consume them recursively so
            // `not not true` and a prefix on a binary RHS (`true and not x`)
            // use the same term boundaries as the runtime Pratt evaluator.
            let mut result = eval_static_term_with_prefix(
                first,
                &mut iter,
                player,
                symbols,
            )?;
            while let Some(op) = iter.next() {
                let right = iter.next()
                    .ok_or_else(|| ScriptError::new("Expected right operand".to_string()))?;
                // `obj_prop`'s right operand is a property NAME, not a value — take its
                // source text and skip evaluation (evaluating it would try to resolve
                // e.g. `number` as a variable).
                if op.as_rule() == Rule::obj_prop {
                    let prop_name = right.as_str().trim().to_string();
                    let prop_symbol = symbols.intern(&prop_name);
                    result = crate::player::script::get_obj_prop(
                        player,
                        symbols,
                        &result,
                        prop_symbol,
                    )?;
                    continue;
                }
                let right_ref = eval_static_term_with_prefix(
                    right,
                    &mut iter,
                    player,
                    symbols,
                )?;
                match op.as_rule() {
                    Rule::join => {
                        // String concatenation (&)
                        let left_str = checked_static_datum(player, &result, symbols)?.string_value(symbols)?;
                        let right_str = checked_static_datum(player, &right_ref, symbols)?.string_value(symbols)?;
                        result = player.alloc_datum(Datum::String(format!("{}{}", left_str, right_str)));
                    }
                    Rule::join_pad => {
                        // Padded concatenation (&&)
                        let left_str = checked_static_datum(player, &result, symbols)?.string_value(symbols)?;
                        let right_str = checked_static_datum(player, &right_ref, symbols)?.string_value(symbols)?;
                        result = player.alloc_datum(Datum::String(format!("{} {}", left_str, right_str)));
                    }
                    // Arithmetic. `.value` on a text member is how movies ship data
                    // tables, and those tables contain expressions, not just literals —
                    // dkbarrel's animation lists start `[nam_DIDDYO+0, point(327, -63),
                    // …]`, offsetting a base member number. Reuse the same datum
                    // operations the runtime evaluator uses so Lingo's semantics
                    // (int/float promotion, list and point recursion) stay identical
                    // between the two paths.
                    Rule::add | Rule::subtract | Rule::multiply | Rule::divide => {
                        let rule = op.as_rule();
                        let v = match rule {
                                Rule::add => {
                                    let (l, r) = (checked_static_datum(player, &result, symbols)?, checked_static_datum(player, &right_ref, symbols)?);
                                    crate::player::datum_operations::add_datums(l, r, player, symbols)?
                                }
                                Rule::subtract => {
                                    let (l, r) = (checked_static_datum(player, &result, symbols)?, checked_static_datum(player, &right_ref, symbols)?);
                                    crate::player::datum_operations::subtract_datums(l, r, player, symbols)?
                                }
                                Rule::multiply => {
                                    checked_static_datum(player, &result, symbols)?;
                                    checked_static_datum(player, &right_ref, symbols)?;
                                    crate::player::datum_operations::multiply_datums(result.clone(), right_ref.clone(), player, symbols)?
                                }
                                _ => {
                                    checked_static_datum(player, &result, symbols)?;
                                    checked_static_datum(player, &right_ref, symbols)?;
                                    crate::player::datum_operations::divide_datums(result.clone(), right_ref.clone(), player, symbols)?
                                }
                            };
                        result = player.alloc_datum(v);
                    }
                    _ => {
                        return Err(ScriptError::new(format!(
                            "Unsupported operator {:?} in static expression", op.as_rule()
                        )));
                    }
                }
            }
            Ok(result)
        },
        Rule::term_arg => {
            let inner = pair.into_inner().next()
                .ok_or_else(|| ScriptError::new("Expected term_arg content".to_string()))?;
            eval_lingo_pair_static(inner, player, symbols)
        },
        Rule::list => {
            let inner = pair.into_inner().next()
                .ok_or_else(|| ScriptError::new("Expected list content".to_string()))?;
            eval_lingo_pair_static(inner, player, symbols)
        },
        Rule::multi_list => {
            let mut result_vec = VecDeque::new();
            for inner_pair in pair.into_inner() {
                let result = eval_lingo_pair_static(inner_pair, player, symbols)?;
                result_vec.push_back(result);
            }
            {
                Ok(player.alloc_datum(Datum::List(DatumType::List, result_vec, false)))
            }
        }
        Rule::string => {
            let str_val = pair.into_inner().next()
                .ok_or_else(|| ScriptError::new("Expected string content".to_string()))?
                .as_str();
            Ok(player.alloc_datum(Datum::String(str_val.to_owned())))
        }
        Rule::prop_list => {
            let inner = pair.into_inner().next()
                .ok_or_else(|| ScriptError::new("Expected prop list content".to_string()))?;
            eval_lingo_pair_static(inner, player, symbols)
        },
        Rule::multi_prop_list => {
            let mut result_vec = VecDeque::new();
            for inner_pair in pair.into_inner() {
                let mut pair_inner = inner_pair.into_inner();
                let key = eval_lingo_pair_static(pair_inner.next()
                    .ok_or_else(|| ScriptError::new("Expected prop list key".to_string()))?, player, symbols)?;
                let value = eval_lingo_pair_static(pair_inner.next()
                    .ok_or_else(|| ScriptError::new("Expected prop list value".to_string()))?, player, symbols)?;

                result_vec.push_back((key, value));
            }
            Ok(player.alloc_datum(Datum::PropList(result_vec, false)))
        }
        Rule::empty_prop_list => {
            Ok(player.alloc_datum(Datum::PropList(VecDeque::new(), false)))
        }
        Rule::number_int => {
            let val = pair.as_str().parse::<i32>()
                .map_err(|e| ScriptError::new(format!("Invalid integer: {}", e)))?;
            Ok(player.alloc_datum(Datum::Int(val)))
        },
        Rule::number_float => {
            let val = pair.as_str().parse::<f64>()
                .map_err(|e| ScriptError::new(format!("Invalid float: {}", e)))?;
            Ok(player.alloc_datum(Datum::Float(val)))
        },
        Rule::rect => {
            let mut inner = pair.into_inner();
            let x_ref = eval_lingo_pair_static(inner.next().ok_or_else(|| ScriptError::new("Expected rect x".to_string()))?, player, symbols)?;
            let y_ref = eval_lingo_pair_static(inner.next().ok_or_else(|| ScriptError::new("Expected rect y".to_string()))?, player, symbols)?;
            let w_ref = eval_lingo_pair_static(inner.next().ok_or_else(|| ScriptError::new("Expected rect w".to_string()))?, player, symbols)?;
            let h_ref = eval_lingo_pair_static(inner.next().ok_or_else(|| ScriptError::new("Expected rect h".to_string()))?, player, symbols)?;
            {
                let x_datum = checked_static_datum(player, &x_ref, symbols)?;
                let y_datum = checked_static_datum(player, &y_ref, symbols)?;
                let w_datum = checked_static_datum(player, &w_ref, symbols)?;
                let h_datum = checked_static_datum(player, &h_ref, symbols)?;
                let rect = Datum::build_rect(&x_datum, &y_datum, &w_datum, &h_datum)?;
                Ok(player.alloc_datum(rect))
            }
        }
        Rule::rgb_num_color => {
            let mut inner = pair.into_inner();
            {
                // Trim each component: Director accepts whitespace inside the
                // parens, e.g. `rgb( 255, 255, 255 )` (spectral-wizard's
                // customHyperlink behavior params are authored this way). Parse
                // as i32 then clamp to 0..=255 to tolerate negatives / >255,
                // matching the LingoExpr-building rgb parser below.
                let mut comp = |label: &str| -> Result<u8, ScriptError> {
                    let s = inner
                        .next()
                        .ok_or_else(|| ScriptError::new(format!("Expected {} component", label)))?
                        .as_str()
                        .trim();
                    let v = s
                        .parse::<i32>()
                        .map_err(|e| ScriptError::new(format!("Invalid {}: {}", label, e)))?;
                    Ok(v.clamp(0, 255) as u8)
                };
                let r = comp("red")?;
                let g = comp("green")?;
                let b = comp("blue")?;
                Ok(player.alloc_datum(Datum::ColorRef(ColorRef::Rgb(r, g, b))))
            }
        }
        Rule::rgb_str_color => {
            let mut inner = pair.into_inner();
            let str_inner = inner.next()
                .ok_or_else(|| ScriptError::new("Expected rgb string".to_string()))?
                .into_inner().next()
                .ok_or_else(|| ScriptError::new("Expected rgb string content".to_string()))?;
            let str_val = str_inner.as_str();
            {
                Ok(player.alloc_datum(Datum::ColorRef(ColorRef::from_hex(str_val))))
            }
        }
        Rule::rgb_color => {
            let mut inner = pair.into_inner();
            if let Some(inner_pair) = inner.next() {
                // recursively call the static evaluator
                eval_lingo_pair_static(inner_pair, player, symbols)
            } else {
                // fallback to default static behavior
                Ok(player.alloc_datum(Datum::Void))
            }
        }
        Rule::symbol => {
            let str_val = pair.into_inner().next()
                .ok_or_else(|| ScriptError::new("Expected symbol name".to_string()))?
                .as_str();
            Ok(player.alloc_datum(Datum::Symbol(symbols.intern(str_val))))
        }
        Rule::bool_true => Ok(player.alloc_datum(datum_bool(true))),
        Rule::bool_false => Ok(player.alloc_datum(datum_bool(false))),
        Rule::void => Ok(DatumRef::Void),
        Rule::string_empty => {
            Ok(player.alloc_datum(Datum::String("".to_owned())))
        }
        Rule::return_const => {
            Ok(player.alloc_datum(Datum::String("\r\n".to_owned())))
        }
        Rule::nohash_symbol => {
            Ok(player.alloc_datum(Datum::Symbol(symbols.intern(pair.as_str()))))
        },
        Rule::point => {
            let mut inner = pair.into_inner();
            let x_ref = eval_lingo_pair_static(inner.next().ok_or_else(|| ScriptError::new("Expected point x".to_string()))?, player, symbols)?;
            let y_ref = eval_lingo_pair_static(inner.next().ok_or_else(|| ScriptError::new("Expected point y".to_string()))?, player, symbols)?;
            {
                let x_datum = checked_static_datum(player, &x_ref, symbols)?;
                let y_datum = checked_static_datum(player, &y_ref, symbols)?;
                let point = Datum::build_point(&x_datum, &y_datum)?;
                Ok(player.alloc_datum(point))
            }
        }
        Rule::empty_list => {
            Ok(player.alloc_datum(Datum::List(DatumType::List, VecDeque::new(), false)))
        },
        Rule::the_prop => {
            // For multi-word properties like "the long time", we need to get the full text
            // and extract the property name
            let full_text = pair.as_str();
            let prop_name = if full_text.starts_with("the ") || full_text.starts_with("THE ") || full_text.starts_with("The ") {
                &full_text[4..]  // Skip "the "
            } else {
                // Shouldn't happen with correct grammar, but handle it
                full_text
            };
            {
                let prop_symbol = symbols.intern(prop_name);
                let prop_value = player.get_movie_prop(symbols, prop_symbol)?;
                Ok(prop_value)
            }
        }
        // `sprite(N)` — dkbarrel's score tables address the stage directly, e.g.
        // `[cmd_Zset, sprite(guispr).locz-1, DKSPR, …]`, so a data table read with
        // `.value` needs sprite references as well as member references. The sprite
        // number is itself an expression (here the global `guispr`).
        Rule::sprite_ref => {
            let inner = pair.into_inner().next()
                .ok_or_else(|| ScriptError::new("Expected sprite number".to_string()))?;
            let num_ref = eval_lingo_pair_static(inner, player, symbols)?;
            {
                let n = checked_static_datum(player, &num_ref, symbols)?.int_value()?;
                Ok(player.alloc_datum(Datum::SpriteRef(n as i16)))
            }
        }
        Rule::member_ref => {
            let mut inner = pair.into_inner();

            // First expression is the member name or number
            let member_expr = inner.next()
                .ok_or_else(|| ScriptError::new("Expected member identifier".to_string()))?;
            let member_id_ref = eval_lingo_pair_static(member_expr, player, symbols)?;

            // Optional: "of castLib X"
            let cast_lib_ref = if let Some(castlib_expr) = inner.next() {
                Some(eval_lingo_pair_static(castlib_expr, player, symbols)?)
            } else {
                None
            };

            {
                let member_id_datum = checked_static_datum(player, &member_id_ref, symbols)?;

                // Get cast lib datum if specified
                let cast_lib_datum = cast_lib_ref
                    .as_ref()
                    .map(|r| checked_static_datum(player, r, symbols))
                    .transpose()?;

                // Use find_member_ref_by_identifiers for proper member lookup
                // This handles both string names and numeric member IDs
                let member_result = player.movie.cast_manager.find_member_ref_by_identifiers(
                    symbols,
                    &member_id_datum,
                    cast_lib_datum.as_ref(),
                    &player.allocator,
                )?;

                let member_ref = match member_result {
                    Some(r) => r,
                    None => {
                        // If cast_lib was specified, create a ref with the specified values
                        // Otherwise return invalid ref
                        if let Some(cast_datum) = cast_lib_datum {
                            let cast_lib_num = match &cast_datum {
                                Datum::Int(num) => *num,
                                Datum::CastLib(num) => *num as i32,
                                Datum::String(name) => {
                                    player.movie.cast_manager.get_cast_by_name(name)
                                        .map(|c| c.number as i32)
                                        .unwrap_or(0)
                                }
                                _ => return Err(ScriptError::new(format!(
                                    "Expected int, string, or castLib, got {:?}",
                                    cast_datum.type_enum()
                                ))),
                            };
                            let member_num = member_id_datum.int_value().unwrap_or(0);
                            super::cast_lib::CastMemberRef {
                                cast_lib: cast_lib_num,
                                cast_member: member_num,
                            }
                        } else {
                            INVALID_CAST_MEMBER_REF
                        }
                    }
                };

                Ok(player.alloc_datum(Datum::CastMember(member_ref)))
            }
        }
        Rule::castlib_ref => {
            let mut inner = pair.into_inner();
            
            let castlib_expr = inner.next()
                .ok_or_else(|| ScriptError::new("Expected castLib identifier".to_string()))?;
            let castlib_ref = eval_lingo_pair_static(castlib_expr, player, symbols)?;
            
            {
                let castlib_num = checked_static_datum(player, &castlib_ref, symbols)?.int_value()
                    .or_else(|_| -> Result<i32, ScriptError> {
                        // If it's not an int, try to get it as a string (named castLib)
                        let name = checked_static_datum(player, &castlib_ref, symbols)?.string_value(symbols)?;
                        // Convert castLib name to number
                        let cast = player
                            .movie
                            .cast_manager
                            .get_cast_by_name(&name)
                            .ok_or_else(|| ScriptError::new(format!("CastLib not found: {}", name)))?;
                        Ok(cast.number as i32)
                    })?;
                
                // Return a CastLib reference datum
                Ok(player.alloc_datum(Datum::CastLib(castlib_num as u32)))
            }
        }
        Rule::lang_ident => {
            // Handle well-known Lingo constants in static context
            let name = pair.as_str();
            match name {
                "QUOTE" => {
                    Ok(player.alloc_datum(Datum::String("\"".to_owned())))
                },
                "TAB" => {
                    Ok(player.alloc_datum(Datum::String("\t".to_owned())))
                },
                // Otherwise it's a variable reference. Director's `value()`
                // evaluates the string as Lingo, so identifiers resolve against
                // the accessible context — NabiscoWorld Mini Mini-Golf stores
                // score-driver data as `["staple 1", obstacle_spr, -1, ...]` in a
                // text member and reads it with `member(nam).text.value`, where
                // `obstacle_spr` is a declared global. Erroring on the identifier
                // failed the whole expression, so `.value` handed back the raw
                // string and `count()` then raised on a string.
                //
                // An identifier that isn't a known global yields VOID rather than
                // an error: the 11.5 dictionary says of `value()` that
                // expressions Lingo cannot parse "will produce unexpected
                // results, but will not produce Lingo errors", and Director reads
                // an unset global as VOID. Lingo identifiers are case-insensitive.
                // Resolve like Director: `value()` evaluates in the CALLING context,
                // so the calling handler's LOCALS are visible, not just globals —
                // "Any Lingo expression that can be put in the Message window or set
                // as the value of a variable can also be used with value()".
                //
                // dkbarrel depends on it. `Movie Driver.SetupSideMovies` does
                //     loopDiddy = 1234567890            -- a LOCAL, no global decl
                //     dlst = member("diddyKani").text.value
                //     loopDiddy = getPos(dlst, 0.0) - 1
                //     dlst = member("diddyKani").text.value
                // so the parsed list embeds the loop-jump index. Resolving globals
                // only put VOID there; `RunAni2`'s
                //     repeat while lst[ani_index] <> cmd_frame
                // then took `ani_index = lst[ani_index+1]` = VOID, walked off the end
                // of the list and never matched the terminator — an infinite loop that
                // froze the movie.
                //
                // get_eval_top_level_prop already implements the whole chain (locals,
                // then `me`, then globals, then top-level props), so defer to it and
                // keep VOID for a name that resolves nowhere.
                _ => {
                    // Locals first (the chain above), then a CASE-INSENSITIVE global
                    // scan. Lingo identifiers are case-insensitive, but the globals map
                    // is keyed by the casing first seen — dkbarrel's tables reference
                    // `cmd_jamfrm` while the global was created as `cmd_JamFrm`, and an
                    // exact-match lookup returned VOID for it.
                    let primary = match get_eval_top_level_prop_classified(player, symbols, name)? {
                        EvalLookupResult::Found(value) => Some(value),
                        EvalLookupResult::OrdinaryError(_) => None,
                    };
                    if let Some(r) = primary {
                        if !matches!(checked_static_datum(player, &r, symbols)?, Datum::Void) {
                            return Ok(r);
                        }
                    }
                    for (key, value) in &player.globals {
                        let matches = symbols
                            .lower(key)
                            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
                            .eq_ignore_ascii_case(name);
                        if !matches {
                            continue;
                        }
                        checked_static_datum(player, &value, symbols)?;
                        return Ok(value.clone());
                    }
                    Ok(DatumRef::Void)
                },
            }
        }
        Rule::config_key | Rule::config_ident_part => {
            // Config keys treated as strings in static context
            {
                Ok(player.alloc_datum(Datum::String(pair.as_str().to_owned())))
            }
        }
        Rule::handler_call => {
            // Support common constructors in static context (e.g. vector(0, 0, -1))
            let mut inner = pair.into_inner();
            let handler_name = inner.next()
                .ok_or_else(|| ScriptError::new("Expected handler name".to_string()))?
                .as_str().to_lowercase();
            let mut arg_refs = vec![];
            if let Some(args_container) = inner.next() {
                for arg_pair in args_container.into_inner() {
                    // Each arg_pair contains an expr; recursively evaluate
                    let mut arg_inner = arg_pair.into_inner();
                    if let Some(expr_pair) = arg_inner.next() {
                        arg_refs.push(eval_lingo_pair_static(expr_pair, player, symbols)?);
                    }
                }
            }
            match handler_name.as_str() {
                "vector" => {
                    {
                        let x = if arg_refs.len() > 0 { checked_static_datum(player, &arg_refs[0], symbols)?.to_float().unwrap_or(0.0) } else { 0.0 };
                        let y = if arg_refs.len() > 1 { checked_static_datum(player, &arg_refs[1], symbols)?.to_float().unwrap_or(0.0) } else { 0.0 };
                        let z = if arg_refs.len() > 2 { checked_static_datum(player, &arg_refs[2], symbols)?.to_float().unwrap_or(0.0) } else { 0.0 };
                        Ok::<DatumRef, ScriptError>(player.alloc_datum(Datum::Vector([x, y, z])))
                    }
                }
                "rect" => {
                    {
                        let l = if arg_refs.len() > 0 { checked_static_datum(player, &arg_refs[0], symbols)?.to_float().unwrap_or(0.0) } else { 0.0 };
                        let t = if arg_refs.len() > 1 { checked_static_datum(player, &arg_refs[1], symbols)?.to_float().unwrap_or(0.0) } else { 0.0 };
                        let r = if arg_refs.len() > 2 { checked_static_datum(player, &arg_refs[2], symbols)?.to_float().unwrap_or(0.0) } else { 0.0 };
                        let b = if arg_refs.len() > 3 { checked_static_datum(player, &arg_refs[3], symbols)?.to_float().unwrap_or(0.0) } else { 0.0 };
                        Ok::<DatumRef, ScriptError>(player.alloc_datum(Datum::Rect([l, t, r, b], 0)))
                    }
                }
                _ => Err(ScriptError::new(format!(
                    "Unsupported handler '{}' in static expression", handler_name
                ))),
            }
        }
        _ => Err(ScriptError::new(format!(
            "Invalid static Lingo expression {:?}",
            inner_rule
        ))),
    }
}

fn parse_number_value(pair: Pair<Rule>) -> Result<f64, ScriptError> {
    match pair.as_rule() {
        Rule::number_int => {
            pair.as_str().parse::<f64>()
                .map_err(|e| ScriptError::new(format!("Invalid number: {}", e)))
        }
        Rule::number_float => {
            pair.as_str().parse::<f64>()
                .map_err(|e| ScriptError::new(format!("Invalid number: {}", e)))
        }
        _ => Err(ScriptError::new(format!("Expected number, got {:?}", pair.as_rule())))
    }
}

/// For a bare-identifier assignment executed via `do`/eval, return the current
/// scope's receiver instance IF it declares `ident_name` as a property — so the
/// assignment lands on the behavior's property rather than a global (Director
/// scoping). Mirrors the identifier READ resolution and only matches a genuinely
/// declared instance property (not `ancestor`/`script`/`ilk` or global names).
fn current_do_receiver_with_prop(
    player: &DirPlayer,
    symbols: &mut SymbolTable,
    ident_name: &str,
) -> Result<Option<crate::player::script_ref::ScriptInstanceRef>, ScriptError> {
    use crate::player::allocator::ScriptInstanceAllocatorTrait;
    if player.scope_count == 0 || (player.scope_count as usize) > player.scopes.len() {
        return Ok(None);
    }
    let raw = if player.current_breakpoint.is_some() {
        player.eval_scope_index.unwrap_or(player.scope_count - 1)
    } else {
        player.scope_count - 1
    };
    let Some(scope) = player.scopes.get(raw as usize) else {
        return Ok(None);
    };
    let Some(receiver) = scope.receiver.clone() else {
        return Ok(None);
    };
    let Some(inst) = player.allocator.get_script_instance_opt(&receiver) else {
        return Err(ScriptError::new_code(
            crate::player::ScriptErrorCode::InvalidReference,
            "Foreign or stale receiver reference".to_string(),
        ));
    };
    let prop_name = symbols.intern(ident_name);
    if inst.properties.contains_key(&prop_name) {
        Ok(Some(receiver))
    } else {
        Ok(None)
    }
}

enum EvalLookupResult {
    Found(DatumRef),
    OrdinaryError(ScriptError),
}

/// Preserve ordinary lookup failures for the static evaluator's legacy VOID
/// fallback while keeping owner violations fatal. Existing async callers use
/// the flattened compatibility boundary below.
fn get_eval_top_level_prop(
    player: &mut DirPlayer,
    symbols: &mut SymbolTable,
    prop_name: &str,
) -> Result<DatumRef, ScriptError> {
    match get_eval_top_level_prop_classified(player, symbols, prop_name)? {
        EvalLookupResult::Found(value) => Ok(value),
        EvalLookupResult::OrdinaryError(_) => {
            // Classic Director resolves unknown identifiers to VOID. Keep the
            // debugger's useful diagnostic while hiding the internal
            // GetSetUtils top-level lookup error from normal evaluation.
            if player.current_breakpoint.is_some() {
                Err(ScriptError::new(format!("Undefined variable: {}", prop_name)))
            } else {
                Ok(DatumRef::Void)
            }
        }
    }
}

fn get_eval_top_level_prop_classified(
    player: &mut DirPlayer,
    symbols: &mut SymbolTable,
    prop_name: &str,
) -> Result<EvalLookupResult, ScriptError> {
    use crate::player::allocator::ScriptInstanceAllocatorTrait;

    if prop_name.starts_with("the ") {
        let actual_prop = &prop_name[4..];
        // `the paramCount` reads from the current handler's scope, not the
        // movie. Bytecode handles it via the_built_in; route AST eval too so
        // the message window and `do` strings can read it.
        if actual_prop.eq_ignore_ascii_case("paramCount") {
            if player.scope_count > 0 && (player.scope_count as usize) <= player.scopes.len() {
                let scope_idx = (player.scope_count - 1) as usize;
                if let Some(scope) = player.scopes.get(scope_idx) {
                    return Ok(EvalLookupResult::Found(
                        player.alloc_datum(Datum::Int(scope.args.len() as i32)),
                    ));
                }
            }
            return Ok(EvalLookupResult::Found(
                player.alloc_datum(Datum::Int(0)),
            ));
        }
        let prop_symbol = symbols.intern(actual_prop);
        let result = player.get_movie_prop(symbols, prop_symbol)?;
        checked_static_datum(player, &result, symbols)?;
        return Ok(EvalLookupResult::Found(result));
    }

    // Resolve against the current scope (locals, args, receiver properties)
    // This is needed both during breakpoint evaluation and during `do` command execution
    if player.scope_count > 0 && (player.scope_count as usize) <= player.scopes.len() {
        let raw = if player.current_breakpoint.is_some() {
            player.eval_scope_index.unwrap_or(player.scope_count - 1)
        } else {
            player.scope_count - 1
        };
        // Defensive: if scope_count was corrupted (e.g. an underflowed u32
        // from a stale post-reset pop), fall through to the globals branch
        // instead of panicking on an out-of-range scope index.
        let scope_idx = raw as usize;
        if scope_idx >= player.scopes.len() {
            // Fall through to globals.
        } else {
            // Check locals by reverse-looking up the name_id from the name table
            {
                let script_ref_for_locals = player.scopes[scope_idx].script_ref.clone();
                if let Some(script_rc) = player.movie.cast_manager.get_script_by_ref(&script_ref_for_locals) {
                    if let Some(lctx) = get_lctx_for_script(player, &script_rc) {
                        if let Some(name_id) = lctx.names.iter().position(|n| n.eq_ignore_ascii_case(prop_name)) {
                            // `locals` is slot-indexed, so the name id has to
                            // be mapped through the HANDLER's local table to
                            // reach a slot. That mapping also enforces "this
                            // handler declares it".
                            //
                            // The `local_is_assigned` check is what preserves
                            // the old hash map's semantics: a key that was
                            // never inserted meant "not a local here", and the
                            // caller falls through to `me` and then globals. A
                            // dense vector has every declared slot present
                            // from the start, so without this a declared but
                            // never-assigned local would silently shadow a
                            // same-named global.
                            let slot = player
                                .movie
                                .cast_manager
                                .get_script_by_ref(&script_ref_for_locals)
                                .and_then(|s| s.get_own_handler_by_local_name_id(
                                    player.scopes[scope_idx].handler_name_id,
                                ))
                                .and_then(|h| {
                                    h.local_name_ids.iter().position(|&nid| nid as usize == name_id)
                                });
                            if let Some(slot) = slot {
                                let local = &player.scopes[scope_idx];
                                if local.local_is_assigned(slot) {
                                    crate::player::interp_stats::record_eval_local_hit();
                                    let value = local.local(slot).clone();
                                    validate_static_stack_datum(player, &value, symbols)?;
                                    return Ok(EvalLookupResult::Found(value.into_ref_with(
                                        &mut player.allocator,
                                        &mut player.bitmap_manager,
                                    )));
                                }
                            }
                        }
                    }
                }
            }

            let scope = &player.scopes[scope_idx];

            // Check "me" (the receiver)
            if prop_name == "me" {
                if let Some(receiver) = scope.receiver.clone() {
                    player
                        .allocator
                        .get_script_instance_opt(&receiver)
                        .ok_or_else(|| {
                            ScriptError::new_code(
                                crate::player::ScriptErrorCode::InvalidReference,
                                "Foreign or stale receiver reference".to_string(),
                            )
                        })?;
                    return Ok(EvalLookupResult::Found(
                        player.alloc_datum(Datum::ScriptInstanceRef(receiver)),
                    ));
                }
            }

            // Resolve handler name from the scope's handler_name_id
            let script_ref = scope.script_ref.clone();
            let handler_name_id = scope.handler_name_id;
            if let Some(script_rc) = player.movie.cast_manager.get_script_by_ref(&script_ref) {
                let script = script_rc.clone();
                // Find the handler whose name_id matches this scope's handler_name_id
                let handler_name = script.handlers.iter()
                    .find(|(_, h)| h.name_id == handler_name_id)
                    .map(|(name, _)| {
                        symbols
                            .display(name)
                            .map(str::to_owned)
                            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)
                    })
                    .transpose()?;
                if let Some(handler_name) = handler_name {
                    if let Some(handler_def) = script.get_own_handler(symbols.intern(&handler_name)) {
                        let handler_def = handler_def.clone();
                        // Check handler arguments by name
                        if let Some(lctx) = get_lctx_for_script(player, &script) {
                            for (i, &name_id) in handler_def.argument_name_ids.iter().enumerate() {
                                if let Some(name) = lctx.names.get(name_id as usize) {
                                    if name.eq_ignore_ascii_case(prop_name) {
                                        if let Some(arg_ref) = player.scopes[scope_idx].args.get(i) {
                                            checked_static_datum(player, arg_ref, symbols)?;
                                            return Ok(EvalLookupResult::Found(arg_ref.clone()));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Check properties on the receiver (me) object
            let receiver = player.scopes[scope_idx].receiver.clone();
            if let Some(receiver_ref) = receiver {
                let prop_name_symbol = symbols.intern(prop_name);
                if let Some(result) = script_get_prop_opt(
                    player,
                    symbols,
                    &receiver_ref,
                    prop_name_symbol,
                )? {
                    checked_static_datum(player, &result, symbols)?;
                    return Ok(EvalLookupResult::Found(result));
                }
            }
        }
    }

    let prop_symbol = symbols.intern(prop_name);
    let global_ref = player.globals.get(&prop_symbol).cloned();
    if let Some(global_ref) = global_ref {
        checked_static_datum(player, &global_ref, symbols)?;
        Ok(EvalLookupResult::Found(global_ref))
    } else {
        match GetSetUtils::get_top_level_prop(player, symbols, prop_symbol) {
            Ok(result) => {
                crate::player::compare::validate_direct_symbol_fields(&result, symbols)?;
                Ok(EvalLookupResult::Found(player.alloc_datum(result)))
            }
            Err(error) => Ok(EvalLookupResult::OrdinaryError(error)),
        }
    }
}

fn parse_lingo_expr_runtime(
    pairs: Pairs<'_, Rule>,
    pratt: &PrattParser<Rule>,
) -> Result<LingoExpr, ScriptError> {
    pratt
        .map_primary(|pair| parse_lingo_rule_runtime(pair, pratt))
        .map_prefix(|op, rhs| match op.as_rule() {
            Rule::not_op => {
                let right = rhs?;
                Ok(LingoExpr::Not(Box::new(right)))
            }
            // Unary minus. Expressed as `x * -1` rather than `0 - x` so it
            // negates every type Director allows it on: `multiply` already
            // recurses into vectors, points and lists, whereas `0 - vector`
            // has no defined arm. Agent Free Ride's vehicle code is full of
            // `-pWorldUp` / `-pLocalDown`.
            Rule::neg_op => {
                let right = rhs?;
                Ok(LingoExpr::Multiply(
                    Box::new(right),
                    Box::new(LingoExpr::IntLiteral(-1)),
                ))
            }
            _ => Err(ScriptError::new(format!(
                "Invalid prefix operator {:?}",
                op.as_rule()
            ))),
        })
        .map_postfix(|lhs, op| match op.as_rule() {
            Rule::list_index => {
                let list_expr = lhs?;
                // Extract the expression inside the brackets
                let index_pairs = op.into_inner();
                let index_expr = parse_lingo_expr_runtime(index_pairs, pratt)?;
                Ok(LingoExpr::ListAccess(Box::new(list_expr), Box::new(index_expr)))
            }
            _ => Err(ScriptError::new(format!(
                "Invalid postfix operator {:?}",
                op.as_rule()
            ))),
        })
        .map_infix(|lhs, op, rhs| match op.as_rule() {
            Rule::add => {
                let left = lhs?;
                let right = rhs?;
                Ok(LingoExpr::Add(Box::new(left), Box::new(right)))
            }
            Rule::subtract => {
                let left = lhs?;
                let right = rhs?;
                Ok(LingoExpr::Subtract(Box::new(left), Box::new(right)))
            }
            Rule::multiply => {
                let left = lhs?;
                let right = rhs?;
                Ok(LingoExpr::Multiply(Box::new(left), Box::new(right)))
            }
            Rule::divide => {
                let left = lhs?;
                let right = rhs?;
                Ok(LingoExpr::Divide(Box::new(left), Box::new(right)))
            }
            Rule::mod_op => {
                let left = lhs?;
                let right = rhs?;
                Ok(LingoExpr::Modulo(Box::new(left), Box::new(right)))
            }
            Rule::join => {
                let left = lhs?;
                let right = rhs?;
                Ok(LingoExpr::Join(Box::new(left), Box::new(right)))
            }
            Rule::join_pad => {
                let left = lhs?;
                let right = rhs?;
                Ok(LingoExpr::JoinPad(Box::new(left), Box::new(right)))
            }
            Rule::and_op => {
                let left = lhs?;
                let right = rhs?;
                Ok(LingoExpr::And(Box::new(left), Box::new(right)))
            }
            Rule::or_op => {
                let left = lhs?;
                let right = rhs?;
                Ok(LingoExpr::Or(Box::new(left), Box::new(right)))
            }
            Rule::eq_op => {
                let left = lhs?;
                let right = rhs?;
                Ok(LingoExpr::Eq(Box::new(left), Box::new(right)))
            }
            Rule::ne_op => {
                let left = lhs?;
                let right = rhs?;
                Ok(LingoExpr::Ne(Box::new(left), Box::new(right)))
            }
            Rule::lt_op => {
                let left = lhs?;
                let right = rhs?;
                Ok(LingoExpr::Lt(Box::new(left), Box::new(right)))
            }
            Rule::gt_op => {
                let left = lhs?;
                let right = rhs?;
                Ok(LingoExpr::Gt(Box::new(left), Box::new(right)))
            }
            Rule::le_op => {
                let left = lhs?;
                let right = rhs?;
                Ok(LingoExpr::Le(Box::new(left), Box::new(right)))
            }
            Rule::ge_op => {
                let left = lhs?;
                let right = rhs?;
                Ok(LingoExpr::Ge(Box::new(left), Box::new(right)))
            }
            Rule::obj_prop => {
                let obj_ref = lhs?;
                let rhs = rhs?;
                match rhs {
                    LingoExpr::Identifier(name) => {
                        let prop_name = name;
                        Ok(LingoExpr::ObjProp(Box::new(obj_ref), prop_name))
                    }
                    LingoExpr::HandlerCall(name, args) => {
                        Ok(LingoExpr::ObjHandlerCall(Box::new(obj_ref), name, args))
                    }
                    LingoExpr::MemberRef(member_expr, _cast_lib) => {
                        // pMapCastLib.member[expr] → ObjHandlerCall(obj, "getPropRef", [#member, expr])
                        // Unwrap single-element ListLiteral (from [expr] syntax)
                        let index_expr = match *member_expr {
                            LingoExpr::ListLiteral(mut items) if items.len() == 1 => items.remove(0),
                            other => other,
                        };
                        Ok(LingoExpr::ObjHandlerCall(
                            Box::new(obj_ref),
                            "getPropRef".to_string(),
                            vec![LingoExpr::SymbolLiteral("member".to_string()), index_expr],
                        ))
                    }
                    _ => Err(ScriptError::new(format!(
                        "Invalid object prop operator rhs {:?}",
                        rhs
                    ))),
                }
            }
            _ => Err(ScriptError::new(format!(
                "Invalid infix operator {:?}",
                op.as_rule()
            ))),
        })
        .parse(pairs)
}

/// Evaluate a dynamic Lingo expression at runtime.
pub fn parse_lingo_rule_runtime(
    pair: Pair<'_, Rule>,
    pratt: &PrattParser<Rule>,
) -> Result<LingoExpr, ScriptError> {
    let inner_rule = pair.as_rule();
    match pair.as_rule() {
        Rule::expr => {
            let inner_pair = pair.into_inner();
            let ast = parse_lingo_expr_runtime(inner_pair, pratt)?;
            Ok(ast)
        }
        Rule::term => {
            let inner_pair = pair.into_inner();
            let ast = parse_lingo_expr_runtime(inner_pair, pratt)?;
            Ok(ast)
        }
        Rule::term_arg => {
            let inner_pair = pair.into_inner();
            let ast = parse_lingo_expr_runtime(inner_pair, pratt)?;
            Ok(ast)
        }
        Rule::multi_list => {
            let mut result_vec = vec![];
            for inner_pair in pair.into_inner() {
                let result = parse_lingo_expr_runtime(inner_pair.into_inner(), pratt)?;
                result_vec.push(result);
            }
            Ok(LingoExpr::ListLiteral(result_vec))
        }
        Rule::string => {
            let str_val = pair.into_inner().next().unwrap().as_str();
            Ok(LingoExpr::StringLiteral(str_val.to_owned()))
        }
        Rule::multi_prop_list => {
            let mut result_vec = vec![];
            for inner_pair in pair.into_inner() {
                let mut pair_inner = inner_pair.into_inner();
                let key = parse_lingo_rule_runtime(pair_inner.next().unwrap(), pratt)?;
                let value =
                    parse_lingo_expr_runtime(pair_inner.next().unwrap().into_inner(), pratt)?;

                result_vec.push((key, value));
            }
            Ok(LingoExpr::PropListLiteral(result_vec))
        }
        Rule::empty_prop_list => Ok(LingoExpr::PropListLiteral(vec![])),
        Rule::number_int => Ok(LingoExpr::IntLiteral(pair.as_str().parse::<i32>().unwrap())),
        Rule::number_float => Ok(LingoExpr::FloatLiteral(
            pair.as_str().parse::<f64>().unwrap(),
        )),
        Rule::rect => {
            let mut inner = pair.into_inner();
            let x_expr = parse_lingo_rule_runtime(inner.next().unwrap(), pratt)?;
            let y_expr = parse_lingo_rule_runtime(inner.next().unwrap(), pratt)?;
            let w_expr = parse_lingo_rule_runtime(inner.next().unwrap(), pratt)?;
            let h_expr = parse_lingo_rule_runtime(inner.next().unwrap(), pratt)?;
            
            Ok(LingoExpr::RectLiteral(vec![(x_expr, y_expr, w_expr, h_expr)]))
        }
        Rule::point => {
            let mut inner = pair.into_inner();
            let x_expr = parse_lingo_rule_runtime(inner.next().unwrap(), pratt)?;
            let y_expr = parse_lingo_rule_runtime(inner.next().unwrap(), pratt)?;
            
            Ok(LingoExpr::PointLiteral(vec![(x_expr, y_expr)]))
        }
        Rule::member_ref => {
            let mut inner = pair.into_inner();
            
            // First expression is the member number
            let member_expr = parse_lingo_rule_runtime(inner.next().unwrap(), pratt)?;
            
            // Optional: "of castLib X"
            let cast_lib_expr = if let Some(castlib_pair) = inner.next() {
                Some(Box::new(parse_lingo_rule_runtime(castlib_pair, pratt)?))
            } else {
                None
            };
            
            Ok(LingoExpr::MemberRef(Box::new(member_expr), cast_lib_expr))
        }
        Rule::sprite_ref => {
            let mut inner = pair.into_inner();
            let sprite_num_pair = inner.next().ok_or_else(|| ScriptError::new("Expected sprite number".to_string()))?;
            let sprite_num_expr = parse_lingo_expr_runtime(sprite_num_pair.into_inner(), pratt)?;
            Ok(LingoExpr::HandlerCall("sprite".to_string(), vec![sprite_num_expr]))
        }
        Rule::field_ref => {
            let mut inner = pair.into_inner();
            let field_arg_pair = inner.next().ok_or_else(|| ScriptError::new("Expected field name or number".to_string()))?;
            let field_arg_expr = parse_lingo_expr_runtime(field_arg_pair.into_inner(), pratt)?;
            Ok(LingoExpr::HandlerCall("field".to_string(), vec![field_arg_expr]))
        }
        // `script "name"` / `script("name")` — the space-separated form is what
        // the documented child-object idiom uses: `new(script "parentName", ..)`.
        Rule::script_ref => {
            let mut inner = pair.into_inner();
            let script_arg_pair = inner.next().ok_or_else(|| ScriptError::new("Expected script name or number".to_string()))?;
            let script_arg_expr = parse_lingo_expr_runtime(script_arg_pair.into_inner(), pratt)?;
            Ok(LingoExpr::HandlerCall("script".to_string(), vec![script_arg_expr]))
        }
        Rule::sprite_of_expr => {
            let mut inner = pair.into_inner();
            let prop_name_pair = inner.next().ok_or_else(|| ScriptError::new("Expected property name".to_string()))?;
            let prop_name = prop_name_pair.as_str().to_string();
            let sprite_pair = inner.next().ok_or_else(|| ScriptError::new("Expected sprite expression".to_string()))?;
            let sprite_expr = parse_lingo_rule_runtime(sprite_pair, pratt)?;
            Ok(LingoExpr::ObjProp(Box::new(sprite_expr), prop_name))
        }
        Rule::castlib_ref => {
            let mut inner = pair.into_inner();
            let castlib_expr = parse_lingo_rule_runtime(inner.next().unwrap(), pratt)?;
            Ok(LingoExpr::HandlerCall("castLib".to_string(), vec![castlib_expr]))
        }
        Rule::castlib_of_expr => {
            let mut inner = pair.into_inner();
            let prop_name_pair = inner.next().ok_or_else(|| ScriptError::new("Expected property name".to_string()))?;
            let prop_name = prop_name_pair.as_str().to_string();
            let castlib_pair = inner.next().ok_or_else(|| ScriptError::new("Expected castLib expression".to_string()))?;
            let castlib_expr = parse_lingo_rule_runtime(castlib_pair, pratt)?;
            Ok(LingoExpr::ObjProp(Box::new(castlib_expr), prop_name))
        }
        Rule::rgb_num_color => {
            let mut inner = pair.into_inner();
            let r_str = inner.next().unwrap().as_str().trim();
            let g_str = inner.next().unwrap().as_str().trim();
            let b_str = inner.next().unwrap().as_str().trim();
            // Parse as i32 first to handle negative values and values > 255, then clamp to u8
            let r = r_str.parse::<i32>().unwrap_or(0).clamp(0, 255) as u8;
            let g = g_str.parse::<i32>().unwrap_or(0).clamp(0, 255) as u8;
            let b = b_str.parse::<i32>().unwrap_or(0).clamp(0, 255) as u8;
            Ok(LingoExpr::ColorLiteral(ColorRef::Rgb(r, g, b)))
        }
        Rule::rgb_str_color => {
            let mut inner = pair.into_inner();
            let str_inner = inner.next().unwrap().into_inner().next().unwrap();
            let str_val = str_inner.as_str();
            Ok(LingoExpr::ColorLiteral(ColorRef::from_hex(str_val)))
        }
        Rule::rgb_color => {
            let mut inner = pair.clone().into_inner();
            if let Some(inner_pair) = inner.next() {
                parse_lingo_rule_runtime(inner_pair, pratt)
            } else {
                let s = pair.as_str();
                Ok(LingoExpr::ColorLiteral(ColorRef::from_hex(s)))
            }
        }
        Rule::symbol => {
            let str_val = pair.into_inner().next().unwrap().as_str();
            Ok(LingoExpr::SymbolLiteral(str_val.to_owned()))
        }
        Rule::bool_true => Ok(LingoExpr::BoolLiteral(true)),
        Rule::bool_false => Ok(LingoExpr::BoolLiteral(false)),
        Rule::void => Ok(LingoExpr::VoidLiteral),
        Rule::string_empty => Ok(LingoExpr::StringLiteral("".to_owned())),
        Rule::return_const => Ok(LingoExpr::StringLiteral("\r\n".to_owned())),
        Rule::nohash_symbol => Ok(LingoExpr::SymbolLiteral(pair.as_str().to_owned())),
        Rule::empty_list => Ok(LingoExpr::ListLiteral(vec![])),
        Rule::put_handler_call => {
            // For put_handler_call, "put" is not captured as a child, only handler_call_args is
            let mut inner = pair.into_inner();
            let mut args = vec![];
            
            if let Some(args_container) = inner.next() {
                // This should be handler_call_args
                for arg_pair in args_container.into_inner() {
                    let arg_pairs = arg_pair.into_inner();
                    let arg_val = parse_lingo_expr_runtime(arg_pairs, pratt)?;
                    args.push(arg_val);
                }
            }

            Ok(LingoExpr::HandlerCall("put".to_string(), args))
        }
        Rule::go_inline => {
            // Skip the syntactic-noise keyword nodes (go/to/frame/of/movie).
            // First term_arg is the destination, second (if any) is the
            // movie path.
            let mut args = vec![];
            for child in pair.into_inner() {
                if let Rule::term_arg = child.as_rule() {
                    // Director marker-navigation keywords: `go next` / `go
                    // previous` / `go loop`. As a bareword these parse as an
                    // undefined variable (→ VOID) and the go handler rejects
                    // it (mixmaster does `do("go next")`), so pass the keyword
                    // as a string literal for go() to resolve to the marker.
                    let low = child.as_str().trim().to_ascii_lowercase();
                    if low == "next" || low == "previous" || low == "loop" {
                        args.push(LingoExpr::StringLiteral(low));
                    } else {
                        args.push(parse_lingo_rule_runtime(child, pratt)?);
                    }
                }
            }
            Ok(LingoExpr::HandlerCall("go".to_owned(), args))
        }
        Rule::if_inline => {
            // `if EXPR then COMMAND` — children are the keyword nodes
            // (if_kw, then_kw) interleaved with the condition expression
            // and the body command. The keyword nodes carry no data
            // beyond their text, so skip them and pick up the two
            // non-keyword children.
            let mut content = pair.into_inner().filter(|p| !matches!(p.as_rule(), Rule::if_kw | Rule::then_kw));
            let cond_pair = content
                .next()
                .ok_or_else(|| ScriptError::new("if_inline: missing condition".to_string()))?;
            let body_pair = content
                .next()
                .ok_or_else(|| ScriptError::new("if_inline: missing then-body".to_string()))?;
            let cond = parse_lingo_rule_runtime(cond_pair, pratt)?;
            let mut body = parse_lingo_rule_runtime(body_pair, pratt)?;
            // Same identifier→handler-call promotion the top-level
            // command path does — a bare identifier in command context
            // is a no-arg handler invocation.
            if let LingoExpr::Identifier(name) = body {
                body = LingoExpr::HandlerCall(name, vec![]);
            }
            Ok(LingoExpr::IfThen(Box::new(cond), Box::new(body)))
        }
        Rule::pass_stmt => Ok(LingoExpr::Pass),
        Rule::handler_call | Rule::command_inline => {
            let mut inner = pair.into_inner();
            let handler_name_pair = inner.next().ok_or_else(|| ScriptError::new("Expected handler name".to_string()))?;
            let handler_name = handler_name_pair.as_str();
            let mut args = vec![];
            
            if let Some(args_container) = inner.next() {
                match args_container.as_rule() {
                    Rule::handler_call_args => {
                        // Process expr children (comma-separated in parentheses)
                        for arg_pair in args_container.into_inner() {
                            let arg_pairs = arg_pair.into_inner();
                            let arg_val = parse_lingo_expr_runtime(arg_pairs, pratt)?;
                            args.push(arg_val);
                        }
                    }
                    Rule::command_inline_args_comma => {
                        // Process expr children (comma-separated)
                        for arg_pair in args_container.into_inner() {
                            let arg_pairs = arg_pair.into_inner();
                            let arg_val = parse_lingo_expr_runtime(arg_pairs, pratt)?;
                            args.push(arg_val);
                        }
                    }
                    Rule::command_inline_args_space => {
                        // Process term_arg children (space-separated)
                        for arg_pair in args_container.into_inner() {
                            // arg_pair is a term_arg, recursively process it
                            let arg_val = parse_lingo_rule_runtime(arg_pair, pratt)?;
                            args.push(arg_val);
                        }
                    }
                    Rule::command_inline_args_single => {
                        // Process single expr
                        let expr_pair = args_container.into_inner().next()
                            .ok_or_else(|| ScriptError::new("Expected expr in single arg".to_string()))?;
                        let arg_pairs = expr_pair.into_inner();
                        let arg_val = parse_lingo_expr_runtime(arg_pairs, pratt)?;
                        args.push(arg_val);
                    }
                    _ => {
                        return Err(ScriptError::new(format!(
                            "Unexpected args rule: {:?}",
                            args_container.as_rule()
                        )));
                    }
                }
            }

            Ok(LingoExpr::HandlerCall(handler_name.to_owned(), args))
        }
        Rule::sound_statement => {
            // `sound fadeIn 2, 60` → HandlerCall("sound", [#fadeIn, 2, 60]).
            // The verb word is a literal keyword (a #symbol), NOT a variable —
            // TypeHandlers::sound reads args[0] as the #verb and dispatches to the
            // sound-channel op (same backend as `sound(2).fadeIn(60)`).
            // Skip the `sound_kw` keyword node (carries no data), like if_inline.
            let mut inner = pair
                .into_inner()
                .filter(|p| !matches!(p.as_rule(), Rule::sound_kw));
            let verb = inner
                .next()
                .ok_or_else(|| ScriptError::new("Expected sound verb".to_string()))?
                .as_str();
            let mut args = vec![LingoExpr::SymbolLiteral(verb.to_owned())];
            if let Some(args_container) = inner.next() {
                match args_container.as_rule() {
                    Rule::command_inline_args_comma => {
                        for arg_pair in args_container.into_inner() {
                            args.push(parse_lingo_expr_runtime(arg_pair.into_inner(), pratt)?);
                        }
                    }
                    Rule::command_inline_args_space => {
                        for arg_pair in args_container.into_inner() {
                            args.push(parse_lingo_rule_runtime(arg_pair, pratt)?);
                        }
                    }
                    Rule::command_inline_args_single => {
                        let expr_pair = args_container.into_inner().next().ok_or_else(|| {
                            ScriptError::new("Expected expr in single arg".to_string())
                        })?;
                        args.push(parse_lingo_expr_runtime(expr_pair.into_inner(), pratt)?);
                    }
                    other => {
                        return Err(ScriptError::new(format!(
                            "Unexpected sound args rule: {:?}",
                            other
                        )));
                    }
                }
            }
            Ok(LingoExpr::HandlerCall("sound".to_owned(), args))
        }
        Rule::lang_ident | Rule::ident => {
            Ok(LingoExpr::Identifier(pair.as_str().to_owned()))
        }
        Rule::prop_name => {
            // Property names (including reserved keywords when used after dot)
            Ok(LingoExpr::Identifier(pair.as_str().to_owned()))
        }
        Rule::config_key | Rule::config_ident_part => {
            // Configuration keys (allows asterisks in identifiers)
            Ok(LingoExpr::Identifier(pair.as_str().to_owned()))
        }
        Rule::dotted_ident => {
            // Parse dotted identifiers like "obj.prop.subprop" into nested ObjProp expressions
            let full_str = pair.as_str();
            let parts: Vec<&str> = full_str.split('.').collect();
            
            if parts.is_empty() {
                return Err(ScriptError::new("Empty dotted identifier".to_string()));
            }
            
            // Start with the first identifier
            let mut result = LingoExpr::Identifier(parts[0].to_owned());
            
            // Chain the rest as ObjProp accesses
            for part in &parts[1..] {
                result = LingoExpr::ObjProp(Box::new(result), part.to_string());
            }
            
            Ok(result)
        }
        Rule::assignment_expr => {
            let mut inner = pair.into_inner();

            let first_term = inner.next().ok_or_else(|| ScriptError::new("Expected first term in assignment_expr".to_string()))?;
            let mut result = parse_lingo_rule_runtime(first_term, pratt)?;

            while let Some(next_pair) = inner.next() {
                if next_pair.as_rule() == Rule::obj_prop {
                    if let Some(term_pair) = inner.next() {
                        let prop_name = term_pair.as_str();
                        result = LingoExpr::ObjProp(Box::new(result), prop_name.to_string());
                    }
                } else if next_pair.as_rule() == Rule::list_index {
                    // Bracket indexing: [expr]
                    let index_pairs = next_pair.into_inner();
                    let index_expr = parse_lingo_expr_runtime(index_pairs, pratt)?;
                    result = LingoExpr::ListAccess(Box::new(result), Box::new(index_expr));
                } else {
                    let prop_name = next_pair.as_str();
                    result = LingoExpr::ObjProp(Box::new(result), prop_name.to_string());
                }
            }

            Ok(result)
        }
        Rule::assignment => {
            let mut inner = pair.into_inner();
            let left_pair = inner.next().ok_or_else(|| ScriptError::new("Expected left side of assignment".to_string()))?;
            let right_pair = inner.next().ok_or_else(|| ScriptError::new("Expected right side of assignment".to_string()))?;

            let left_expr = if left_pair.as_rule() == Rule::assignment_expr {
                parse_lingo_rule_runtime(left_pair, pratt)?
            } else {
                match left_pair.as_rule() {
                    Rule::ident | Rule::lang_ident => {
                        let ident_name = left_pair.as_str();
                        LingoExpr::Identifier(ident_name.to_owned())
                    }
                    Rule::dotted_ident => {
                        parse_lingo_rule_runtime(left_pair, pratt)?
                    }
                    _ => parse_lingo_rule_runtime(left_pair, pratt)?,
                }
            };

            let right_expr = parse_lingo_expr_runtime(right_pair.into_inner(), pratt)?;

            Ok(LingoExpr::Assignment(
                Box::new(left_expr),
                Box::new(right_expr),
            ))
        }
        Rule::put_display => {
            let mut inner = pair.into_inner();
            let expr_pair = inner.next().ok_or_else(|| ScriptError::new("Expected expression in put display".to_string()))?;
            let value_expr = parse_lingo_expr_runtime(expr_pair.into_inner(), pratt)?;
            Ok(LingoExpr::PutDisplay(Box::new(value_expr)))
        }
        Rule::put_display_multi => {
            let mut inner = pair.into_inner();
            let mut exprs = vec![];
            for expr_pair in inner {
                let expr = parse_lingo_expr_runtime(expr_pair.into_inner(), pratt)?;
                exprs.push(expr);
            }
            // Multiple comma-separated args means this is a handler call
            Ok(LingoExpr::HandlerCall("put".to_string(), exprs))
        }
        Rule::put_into => {
            let mut inner = pair.into_inner();
            let expr_pair = inner.next().ok_or_else(|| ScriptError::new("Expected expression".to_string()))?;
            let expr = parse_lingo_expr_runtime(expr_pair.into_inner(), pratt)?;
            let target_pair = inner.next().ok_or_else(|| ScriptError::new("Expected target identifier".to_string()))?;
            let target_name = target_pair.as_str().to_string();
            Ok(LingoExpr::PutInto(
                Box::new(expr), 
                Box::new(LingoExpr::Identifier(target_name))
            ))
        }
        Rule::put_before => {
            let mut inner = pair.into_inner();
            let expr_pair = inner.next().ok_or_else(|| ScriptError::new("Expected expression".to_string()))?;
            let expr = parse_lingo_expr_runtime(expr_pair.into_inner(), pratt)?;
            let target_pair = inner.next().ok_or_else(|| ScriptError::new("Expected target identifier".to_string()))?;
            let target_name = target_pair.as_str().to_string();
            Ok(LingoExpr::PutBefore(
                Box::new(expr), 
                Box::new(LingoExpr::Identifier(target_name))
            ))
        },
        Rule::put_after => {
            let mut inner = pair.into_inner();
            let expr_pair = inner.next().ok_or_else(|| ScriptError::new("Expected expression".to_string()))?;
            let expr = parse_lingo_expr_runtime(expr_pair.into_inner(), pratt)?;
            let target_pair = inner.next().ok_or_else(|| ScriptError::new("Expected target identifier".to_string()))?;
            let target_name = target_pair.as_str().to_string();
            Ok(LingoExpr::PutAfter(
                Box::new(expr), 
                Box::new(LingoExpr::Identifier(target_name))
            ))
        },
        Rule::put_into_chunk => {
            let mut inner = pair.into_inner();
            let expr_pair = inner.next().ok_or_else(|| ScriptError::new("Expected expression".to_string()))?;
            let expr = parse_lingo_expr_runtime(expr_pair.into_inner(), pratt)?;
            let chunk_pair = inner.next().ok_or_else(|| ScriptError::new("Expected chunk expression".to_string()))?;
            let chunk = parse_lingo_rule_runtime(chunk_pair, pratt)?;
            Ok(LingoExpr::PutInto(Box::new(expr), Box::new(chunk)))
        },
        Rule::put_before_chunk => {
            let mut inner = pair.into_inner();
            let expr_pair = inner.next().ok_or_else(|| ScriptError::new("Expected expression".to_string()))?;
            let expr = parse_lingo_expr_runtime(expr_pair.into_inner(), pratt)?;
            let chunk_pair = inner.next().ok_or_else(|| ScriptError::new("Expected chunk expression".to_string()))?;
            let chunk = parse_lingo_rule_runtime(chunk_pair, pratt)?;
            Ok(LingoExpr::PutBefore(Box::new(expr), Box::new(chunk)))
        },
        Rule::put_after_chunk => {
            let mut inner = pair.into_inner();
            let expr_pair = inner.next().ok_or_else(|| ScriptError::new("Expected expression".to_string()))?;
            let expr = parse_lingo_expr_runtime(expr_pair.into_inner(), pratt)?;
            let chunk_pair = inner.next().ok_or_else(|| ScriptError::new("Expected chunk expression".to_string()))?;
            let chunk = parse_lingo_rule_runtime(chunk_pair, pratt)?;
            Ok(LingoExpr::PutAfter(Box::new(expr), Box::new(chunk)))
        },
        Rule::put_statement => {
            let mut inner = pair.clone().into_inner();
            if let Some(inner_pair) = inner.next() {
                parse_lingo_rule_runtime(inner_pair, pratt)
            } else {
                parse_lingo_rule_runtime(pair, pratt)
            }
        }
        Rule::set_statement => {
            let mut inner = pair.into_inner();
            let left_pair = inner.next().ok_or_else(|| ScriptError::new("Expected left side of set statement".to_string()))?;
            let right_pair = inner.next().ok_or_else(|| ScriptError::new("Expected right side of set statement".to_string()))?;
            let left_expr = parse_lingo_expr_runtime(left_pair.into_inner(), pratt)?;
            let right_expr = parse_lingo_expr_runtime(right_pair.into_inner(), pratt)?;
            Ok(LingoExpr::Assignment(Box::new(left_expr), Box::new(right_expr)))
        }
        Rule::delete_statement => {
            let mut inner = pair.into_inner();
            let chunk_pair = inner.next().ok_or_else(|| ScriptError::new("Expected chunk expression after delete".to_string()))?;
            let chunk_expr = parse_lingo_rule_runtime(chunk_pair, pratt)?;
            Ok(LingoExpr::DeleteChunk(Box::new(chunk_expr)))
        }
        Rule::chunk_expr => {
            let mut inner = pair.into_inner();
            let chunk_type_pair = inner.next().ok_or_else(|| ScriptError::new("Expected chunk type".to_string()))?;
            let chunk_type = match chunk_type_pair.as_str().to_ascii_lowercase().as_str() {
                "item" => Symbol::builtin(BuiltInSymbol::Item),
                "word" => Symbol::builtin(BuiltInSymbol::Word),
                "char" => Symbol::builtin(BuiltInSymbol::Char),
                "line" => Symbol::builtin(BuiltInSymbol::Line),
                _ => return Err(ScriptError::new("Invalid string chunk type".to_string())),
            };
            let index_pair = inner.next().ok_or_else(|| ScriptError::new("Expected index expression".to_string()))?;
            let index_expr = parse_lingo_expr_runtime(index_pair.into_inner(), pratt)?;
            // Check for optional range: "to <expr>"
            let next_pair = inner.next().ok_or_else(|| ScriptError::new("Expected source expression".to_string()))?;
            let (range_end_expr, source_pair) = if next_pair.as_rule() == Rule::chunk_range {
                let range_inner = next_pair.into_inner().next()
                    .ok_or_else(|| ScriptError::new("Expected range end expression".to_string()))?;
                let range_expr = parse_lingo_expr_runtime(range_inner.into_inner(), pratt)?;
                let src = inner.next().ok_or_else(|| ScriptError::new("Expected source expression after range".to_string()))?;
                (Some(range_expr), src)
            } else {
                (None, next_pair)
            };
            let source_expr = match source_pair.as_rule() {
                Rule::ident | Rule::lang_ident => {
                    // Regular identifier - just use it as-is
                    LingoExpr::Identifier(source_pair.as_str().to_string())
                },
                Rule::the_prop => {
                    // "the X" property - parse it to get the full "the X" form
                    parse_lingo_rule_runtime(source_pair, pratt)?
                },
                Rule::the_prop_of => {
                    // "the X of Y" - parse recursively
                    parse_lingo_rule_runtime(source_pair, pratt)?
                },
                Rule::chunk_expr => {
                    // Nested chunk expression
                    parse_lingo_rule_runtime(source_pair, pratt)?
                },
                _ => parse_lingo_rule_runtime(source_pair, pratt)?,
            };
            Ok(LingoExpr::ChunkExpr(chunk_type, Box::new(index_expr), range_end_expr.map(Box::new), Box::new(source_expr)))
        }
        Rule::the_prop => {
            // For multi-word properties like "the long time", we need to get the full text
            // The full text is already "the property_name" format
            let full_text = pair.as_str();
            // Return an identifier that will be resolved at runtime
            Ok(LingoExpr::Identifier(full_text.to_string()))
        }
        Rule::the_chunk_count => {
            // `the number of <chunk_kind> in/of <expr>` — Director's legacy
            // chunk-count form. Translates to the compound property
            // "number of <kind>" on the target expr; the runtime get_obj_prop
            // path on String resolves it to chunk count.
            let mut inner = pair.into_inner();
            let kind_pair = inner.next().ok_or_else(|| ScriptError::new("Expected chunk kind after 'the number of'".to_string()))?;
            let kind = kind_pair.as_str().to_ascii_lowercase();
            let target_pair = inner.next().ok_or_else(|| ScriptError::new("Expected target after 'in'/'of'".to_string()))?;
            let target_expr = parse_lingo_expr_runtime(target_pair.into_inner(), pratt)?;
            Ok(LingoExpr::ThePropOf(Box::new(target_expr), format!("number of {}", kind)))
        }
        Rule::the_prop_of => {
            let mut inner = pair.into_inner();
            let prop_name_pair = inner.next().ok_or_else(|| ScriptError::new("Expected property name after 'the'".to_string()))?;
            let prop_name = prop_name_pair.as_str().to_string();
            let target_pair = inner.next().ok_or_else(|| ScriptError::new("Expected target after 'of'".to_string()))?;
            
            // Check what kind of target we have
            let target_expr = match target_pair.as_rule() {
                Rule::castlib_of_expr | Rule::sprite_of_expr | Rule::prop_of_expr => {
                    // These are already structured as "X of Y", parse them directly
                    parse_lingo_rule_runtime(target_pair, pratt)?
                }
                _ => {
                    // Regular expression
                    parse_lingo_expr_runtime(target_pair.into_inner(), pratt)?
                }
            };
            
            Ok(LingoExpr::ThePropOf(Box::new(target_expr), prop_name))
        }
        Rule::prop_of_expr => {
            let mut inner = pair.into_inner();
            let prop_name_pair = inner.next().ok_or_else(|| ScriptError::new("Expected property name".to_string()))?;
            let prop_name = prop_name_pair.as_str().to_string();
            let obj_expr_pair = inner.next().ok_or_else(|| ScriptError::new("Expected object expression".to_string()))?;
            let obj_expr = parse_lingo_expr_runtime(obj_expr_pair.into_inner(), pratt)?;
            Ok(LingoExpr::ObjProp(Box::new(obj_expr), prop_name))
        }
        Rule::parens_list => {
            let mut inner = pair.into_inner();
            let mut exprs = vec![];
            for expr_pair in inner {
                let expr = parse_lingo_expr_runtime(expr_pair.into_inner(), pratt)?;
                exprs.push(expr);
            }
            Ok(LingoExpr::ListLiteral(exprs))
        }
        Rule::parens_empty => {
            Ok(LingoExpr::ListLiteral(vec![]))
        }
        _ => Err(ScriptError::new(format!(
            "Invalid runtime Lingo expression {:?}",
            inner_rule
        ))),
    }
}

/// Evaluate common chunk expression components: chunk_type, start index, end index, value string.
fn write_chunk_source(player: &mut DirPlayer, symbols: &mut SymbolTable, source_expr: &LingoExpr, new_string: String) {
    match source_expr {
        LingoExpr::Identifier(name) => {
            let new_ref = player.alloc_datum(Datum::String(new_string));
            player.globals.insert(symbols.intern(name), new_ref);
        },
        LingoExpr::HandlerCall(handler_name, args) if handler_name.eq_ignore_ascii_case("field") => {
            // field(name_or_num) or field(name_or_num, castLib_num)
            let member_name_or_num = args.first().and_then(|arg| match arg {
                LingoExpr::StringLiteral(s) => Some(Datum::String(s.clone())),
                LingoExpr::IntLiteral(n) => Some(Datum::Int(*n)),
                _ => None,
            });
            let cast_id = args.get(1).and_then(|arg| match arg {
                LingoExpr::IntLiteral(n) => Some(Datum::Int(*n)),
                _ => None,
            });
            if let Some(member_id) = member_name_or_num {
                let member_ref = player.movie.cast_manager
                    .find_member_ref_by_identifiers(symbols, &member_id, cast_id.as_ref(), &player.allocator);
                if let Ok(Some(member_ref)) = member_ref {
                    if let Some(member) = player.movie.cast_manager.find_mut_member_by_ref(&member_ref) {
                        use crate::player::cast_member::CastMemberType;
                        match &mut member.member_type {
                            CastMemberType::Field(field) => { field.set_text_preserving_caret(new_string); },
                            CastMemberType::Text(text) => { text.set_text_preserving_caret(new_string); },
                            _ => { log::warn!("put into chunk: source is not a field/text member"); }
                        }
                    }
                }
            }
        },
        _ => {
            log::warn!("put into chunk: cannot write back to non-identifier, non-field source");
        }
    }
}



/// One synchronous evaluator turn. Pending requests own all arguments and are
/// returned to the session before any host work can begin.
pub(crate) enum EvalTurn {
    Complete(Result<DatumRef, ScriptError>),
    Pending { request: EvalPending },
}

/// Parse and register an owned runtime expression. Callers drive the returned
/// continuation through `eval_lingo_expr_turn`; no player or symbol-table
/// borrow crosses a pending request.
pub(crate) fn start_eval_lingo_expr(
    session: &mut crate::player::session::RuntimeSession,
    player_id: crate::player::session::PlayerId,
    expr: String,
) -> Result<EvalId, ScriptError> {
    let ast = parse_lingo_expr_ast_runtime(Rule::eval_expr, expr)?;
    session.start_eval(player_id, ast)
}

pub(crate) fn start_eval_lingo_command(
    session: &mut crate::player::session::RuntimeSession,
    player_id: crate::player::session::PlayerId,
    expr: String,
) -> Result<EvalId, ScriptError> {
    let lines: Vec<String> = expr
        .split(|c| c == '\r' || c == '\n')
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("--"))
        .map(str::to_owned)
        .collect();
    if lines.len() <= 1 {
        let ast = parse_lingo_expr_ast_runtime(Rule::command_eval_expr, expr)?;
        return session.start_eval(player_id, ast);
    }
    session.start_command_eval(player_id, lines)
}

pub(crate) fn eval_lingo_expr_turn(
    session: &mut crate::player::session::RuntimeSession,
    id: EvalId,
) -> EvalTurn {
    session.turn_eval(id)
}

pub(crate) fn complete_eval_lingo_expr(
    session: &mut crate::player::session::RuntimeSession,
    id: EvalId,
    action: &EvalAction,
    owner: &crate::player::ownership::OwnerToken,
    result: Result<DatumRef, ScriptError>,
) -> EvalTurn {
    session.resume_eval(id, action, owner, result)
}

/// Queue one already-classified owner-bound VM request and await its actual
/// completion. The request is retained in the session before this future
/// yields; the owner pump executes it outside the borrow and routes the exact
/// result back through `submit_pending_eval_completion`.
pub(crate) async fn invoke_request_owned(
    session: crate::player::session::RuntimeSessionHandle,
    player_id: crate::player::session::PlayerId,
    owner: crate::player::ownership::OwnerToken,
    request: crate::player::driver::InternalVmRequest,
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
    let (_id, _action, receiver) = session
        .borrow_mut()
        .start_eval_request(player_id, request)?;
    receiver
        .recv()
        .await
        .map_err(|_| crate::player::cancelled_scope_error())
        .and_then(|result| result)
}

/// Invoke a nested script handler while retaining the caller's real driver.
/// The subordinate evaluator owns the child capability; this wrapper only
/// schedules a pending child action and awaits the callback channel outside
/// every `RuntimeSession` borrow.  The returned `ScopeResult` preserves the
/// handler's `pass` outcome for event propagation.
pub(crate) async fn invoke_script_callback_owned(
    session: crate::player::session::RuntimeSessionHandle,
    player_id: crate::player::session::PlayerId,
    owner: crate::player::ownership::OwnerToken,
    receiver: Option<crate::player::ScriptInstanceRef>,
    handler_ref: crate::player::script::ScriptHandlerRef,
    args: Vec<DatumRef>,
    use_raw_arg_list: bool,
) -> Result<crate::player::scope::ScopeResult, ScriptError> {
    let (id, turn, callback) = session.borrow_mut().start_owned_script_callback(
        player_id,
        owner.clone(),
        receiver,
        handler_ref,
        args,
        use_raw_arg_list,
    )?;
    let mut cancellation = CallbackCancellation {
        session: session.clone(),
        player_id,
        owner: owner.clone(),
        id: id.clone(),
        armed: true,
    };
    match turn {
        crate::player::session::EvalRequestTurn::Child(
            crate::player::driver::DriverTurn::Waiting,
        ) => {
            session.borrow_mut().retain_pending_command(crate::player::driver::PendingCommand {
                player_id,
                owner,
                action: None,
                started: false,
                ticket: None,
                completer: None,
                event_sender: None,
                score_continuation: None,
                child_completion: None,
                eval_child: Some(id),
                eval_sender: None,
            });
        }
        crate::player::session::EvalRequestTurn::Child(
            crate::player::driver::DriverTurn::Pending(action),
        ) => {
            session.borrow_mut().retain_pending_command(crate::player::driver::PendingCommand {
                player_id,
                owner,
                ticket: Some(action.ticket().clone()),
                action: Some(action),
                started: false,
                completer: None,
                event_sender: None,
                score_continuation: None,
                child_completion: None,
                eval_child: Some(id),
                eval_sender: None,
            });
        }
        crate::player::session::EvalRequestTurn::Evaluator(
            EvalTurn::Complete(_),
        ) => {}
        crate::player::session::EvalRequestTurn::Child(
            crate::player::driver::DriverTurn::Complete(_),
        )
        | crate::player::session::EvalRequestTurn::Child(
            crate::player::driver::DriverTurn::Error(_),
        ) => {}
        crate::player::session::EvalRequestTurn::Evaluator(EvalTurn::Pending { .. })
        | crate::player::session::EvalRequestTurn::MovieAsync(_)
        | crate::player::session::EvalRequestTurn::Flash(_) => {
            return Err(ScriptError::new(
                "nested script callback yielded an unsupported evaluator request".to_owned(),
            ));
        }
        crate::player::session::EvalRequestTurn::ExternalXtra(_) => {
            return Err(ScriptError::new(
                "external Xtra cannot originate from a prepared child evaluator".to_owned(),
            ));
        }
        crate::player::session::EvalRequestTurn::ExternalXtraLoad(_) => {
            return Err(ScriptError::new(
                "external Xtra load cannot originate from a prepared child evaluator".to_owned(),
            ));
        }
        crate::player::session::EvalRequestTurn::XtraPending(_) => {
            return Err(ScriptError::new(
                "owner-bound Multiuser/Curl operation cannot originate from a prepared child evaluator".to_owned(),
            ));
        }
    }
    let result = callback
        .recv()
        .await
        .map_err(|_| crate::player::cancelled_scope_error())?;
    cancellation.armed = false;
    result
}

struct CallbackCancellation {
    session: crate::player::session::RuntimeSessionHandle,
    player_id: crate::player::session::PlayerId,
    owner: crate::player::ownership::OwnerToken,
    id: EvalId,
    armed: bool,
}

impl Drop for CallbackCancellation {
    fn drop(&mut self) {
        if self.armed {
            self.session
                .borrow_mut()
                .cancel_eval_callback(&self.id, self.player_id, &self.owner);
        }
    }
}

#[derive(Clone, Copy)]
enum EvalBinary {
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    Join(bool),
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
}

#[derive(Clone)]
enum EvalFrame {
    Evaluate(LingoExpr),
    ApplyList(usize),
    ApplyPropList(usize),
    ApplyBinary(EvalBinary),
    ApplyNot,
    ApplyObjProp(String),
    ApplySetProperty(String),
    ApplyHandler { name: String, argc: usize },
    ApplyObjHandler { name: String, argc: usize },
    ApplyListAccess,
    ApplyChunkAccess { property: String, target: LingoExpr },
    ApplyAssignment(LingoExpr),
    ProbeIndexedAssignment { value: DatumRef, target: LingoExpr },
    ApplyIndexedAssignment { value: DatumRef, property: Option<String> },
    ApplySetAtResult(DatumRef),
    ApplyPut { kind: u8, target: LingoExpr },
    ApplyPutChunk { kind: u8, target: LingoExpr, value: DatumRef },
    FormatPutValue(DatumRef),
    ReadChunkSource(LingoExpr),
    ReadChunkSourceDone,
    ConvertIndex,
    ConvertEnd,
    ApplyChunkSource,
    ApplyChunk { chunk_type: Symbol, has_end: bool },
    ApplyDeleteChunk { chunk_type: Symbol, has_end: bool, source: LingoExpr },
    ApplyRect,
    ApplyPoint,
    ApplyMember { has_cast: bool },
    ApplyIf { body: LingoExpr },
}

macro_rules! eval_turn_try {
    ($value:expr) => {
        match $value {
            Ok(value) => value,
            Err(error) => return Some(EvalTurn::Complete(Err(error))),
        }
    };
}

impl EvalContinuation {
    fn pop_value(&mut self) -> Result<DatumRef, ScriptError> {
        self.values
            .pop()
            .ok_or_else(|| ScriptError::new("evaluator operand stack underflow".to_owned()))
    }

    fn pop_values(&mut self, count: usize) -> Result<Vec<DatumRef>, ScriptError> {
        if self.values.len() < count {
            return Err(ScriptError::new("evaluator argument stack underflow".to_owned()));
        }
        let start = self.values.len() - count;
        // Child frames are pushed in reverse order, so the operand stack
        // already contains this suffix in source evaluation order.
        Ok(self.values.split_off(start))
    }

    fn intern(
        session: &mut crate::player::session::RuntimeSession,
        player_id: crate::player::session::PlayerId,
        name: &str,
    ) -> Result<Symbol, ScriptError> {
        session
            .with_player(player_id, |context| Ok::<_, ScriptError>(context.symbols.intern(name)))
            .ok_or_else(crate::player::cancelled_scope_error)?
    }

    fn validate_ref(
        session: &mut crate::player::session::RuntimeSession,
        player_id: crate::player::session::PlayerId,
        value: &DatumRef,
    ) -> Result<DatumRef, ScriptError> {
        session
            .with_player(player_id, |context| {
                crate::player::driver::checked_internal_datum(context.player, context.symbols, value)
                    .map(|_| value.clone())
            })
            .ok_or_else(crate::player::cancelled_scope_error)?
    }

    fn evaluate_frame(
        &mut self,
        session: &mut crate::player::session::RuntimeSession,
        frame: EvalFrame,
    ) -> Option<EvalTurn> {
        let EvalFrame::Evaluate(expr) = frame else {
            self.frames.push(frame);
            return None;
        };
        match expr {
            LingoExpr::SymbolLiteral(name) => {
                let result = Self::intern(session, self.player_id, &name).and_then(|symbol| {
                    session.with_player(self.player_id, |context| {
                        Ok::<_, ScriptError>(context.player.alloc_datum(Datum::Symbol(symbol)))
                    }).ok_or_else(crate::player::cancelled_scope_error)?
                });
                self.values.push(eval_turn_try!(result));
            }
            LingoExpr::StringLiteral(value) => {
                let result = session.with_player(self.player_id, |context| {
                    Ok::<_, ScriptError>(context.player.alloc_datum(Datum::String(value)))
                }).ok_or_else(crate::player::cancelled_scope_error).and_then(|result| result);
                self.values.push(eval_turn_try!(result));
            }
            LingoExpr::VoidLiteral | LingoExpr::Pass => self.values.push(DatumRef::Void),
            LingoExpr::BoolLiteral(value) => {
                let result = session.with_player(self.player_id, |context| {
                    Ok::<_, ScriptError>(context.player.alloc_datum(Datum::Int(i32::from(value))))
                }).ok_or_else(crate::player::cancelled_scope_error).and_then(|result| result);
                self.values.push(eval_turn_try!(result));
            }
            LingoExpr::IntLiteral(value) => {
                let result = session.with_player(self.player_id, |context| {
                    Ok::<_, ScriptError>(context.player.alloc_datum(Datum::Int(value)))
                }).ok_or_else(crate::player::cancelled_scope_error).and_then(|result| result);
                self.values.push(eval_turn_try!(result));
            }
            LingoExpr::FloatLiteral(value) => {
                let result = session.with_player(self.player_id, |context| {
                    Ok::<_, ScriptError>(context.player.alloc_datum(Datum::Float(value)))
                }).ok_or_else(crate::player::cancelled_scope_error).and_then(|result| result);
                self.values.push(eval_turn_try!(result));
            }
            LingoExpr::ColorLiteral(value) => {
                let result = session.with_player(self.player_id, |context| {
                    Ok::<_, ScriptError>(context.player.alloc_datum(Datum::ColorRef(value)))
                }).ok_or_else(crate::player::cancelled_scope_error).and_then(|result| result);
                self.values.push(eval_turn_try!(result));
            }
            LingoExpr::ListLiteral(items) => {
                let len = items.len();
                self.frames.push(EvalFrame::ApplyList(len));
                for item in items.into_iter().rev() { self.frames.push(EvalFrame::Evaluate(item)); }
            }
            LingoExpr::PropListLiteral(pairs) => {
                let len = pairs.len();
                self.frames.push(EvalFrame::ApplyPropList(len));
                for (key, value) in pairs.into_iter().rev() {
                    self.frames.push(EvalFrame::Evaluate(value));
                    self.frames.push(EvalFrame::Evaluate(key));
                }
            }
            LingoExpr::HandlerCall(name, args) => {
                let argc = args.len();
                self.frames.push(EvalFrame::ApplyHandler { name, argc });
                for arg in args.into_iter().rev() { self.frames.push(EvalFrame::Evaluate(arg)); }
            }
            LingoExpr::ObjProp(object, name) => {
                self.frames.push(EvalFrame::ApplyObjProp(name));
                self.frames.push(EvalFrame::Evaluate(*object));
            }
            LingoExpr::ObjHandlerCall(object, name, args) => {
                let argc = args.len();
                self.frames.push(EvalFrame::ApplyObjHandler { name, argc });
                for arg in args.into_iter().rev() { self.frames.push(EvalFrame::Evaluate(arg)); }
                self.frames.push(EvalFrame::Evaluate(*object));
            }
            LingoExpr::ListAccess(list, index) => {
                if let LingoExpr::ObjProp(obj, prop_name) = *list {
                    let prop_lower = prop_name.to_ascii_lowercase();
                    if matches!(prop_lower.as_str(), "line" | "item" | "word" | "char") {
                        let target = LingoExpr::ListAccess(
                            Box::new(LingoExpr::ObjProp(obj.clone(), prop_name.clone())),
                            index.clone(),
                        );
                        self.frames.push(EvalFrame::ApplyChunkAccess { property: prop_lower, target });
                        self.frames.push(EvalFrame::Evaluate(*index));
                        self.frames.push(EvalFrame::Evaluate(*obj));
                    } else {
                        self.frames.push(EvalFrame::ApplyListAccess);
                        self.frames.push(EvalFrame::Evaluate(*index));
                        self.frames.push(EvalFrame::Evaluate(LingoExpr::ObjProp(obj, prop_name)));
                    }
                } else {
                    self.frames.push(EvalFrame::ApplyListAccess);
                    self.frames.push(EvalFrame::Evaluate(*index));
                    self.frames.push(EvalFrame::Evaluate(*list));
                }
            }
            LingoExpr::Assignment(left, right) => {
                self.frames.push(EvalFrame::ApplyAssignment(*left));
                self.frames.push(EvalFrame::Evaluate(*right));
            }
            LingoExpr::Add(a,b) => self.push_binary(EvalBinary::Add, *a, *b),
            LingoExpr::Subtract(a,b) => self.push_binary(EvalBinary::Subtract, *a, *b),
            LingoExpr::Multiply(a,b) => self.push_binary(EvalBinary::Multiply, *a, *b),
            LingoExpr::Divide(a,b) => self.push_binary(EvalBinary::Divide, *a, *b),
            LingoExpr::Modulo(a,b) => self.push_binary(EvalBinary::Modulo, *a, *b),
            LingoExpr::Join(a,b) => self.push_binary(EvalBinary::Join(false), *a, *b),
            LingoExpr::JoinPad(a,b) => self.push_binary(EvalBinary::Join(true), *a, *b),
            LingoExpr::Eq(a,b) => self.push_binary(EvalBinary::Eq, *a, *b),
            LingoExpr::Ne(a,b) => self.push_binary(EvalBinary::Ne, *a, *b),
            LingoExpr::Lt(a,b) => self.push_binary(EvalBinary::Lt, *a, *b),
            LingoExpr::Gt(a,b) => self.push_binary(EvalBinary::Gt, *a, *b),
            LingoExpr::Le(a,b) => self.push_binary(EvalBinary::Le, *a, *b),
            LingoExpr::Ge(a,b) => self.push_binary(EvalBinary::Ge, *a, *b),
            LingoExpr::And(a,b) => self.push_binary(EvalBinary::And, *a, *b),
            LingoExpr::Or(a,b) => self.push_binary(EvalBinary::Or, *a, *b),
            LingoExpr::Not(value) => { self.frames.push(EvalFrame::ApplyNot); self.frames.push(EvalFrame::Evaluate(*value)); }
            LingoExpr::PutInto(value, target) => self.push_put(0, *value, *target),
            LingoExpr::PutBefore(value, target) => self.push_put(1, *value, *target),
            LingoExpr::PutAfter(value, target) => self.push_put(2, *value, *target),
            LingoExpr::PutDisplay(value) => self.push_put(3, *value, LingoExpr::VoidLiteral),
            LingoExpr::ChunkExpr(kind, index, end, source) => {
                self.frames.push(EvalFrame::ApplyChunk { chunk_type: kind, has_end: end.is_some() });
                if let Some(end) = end {
                    self.frames.push(EvalFrame::ConvertEnd);
                    self.frames.push(EvalFrame::Evaluate(*end));
                }
                self.frames.push(EvalFrame::ConvertIndex);
                self.frames.push(EvalFrame::Evaluate(*index));
                self.frames.push(EvalFrame::ApplyChunkSource);
                self.frames.push(EvalFrame::Evaluate(*source));
            }
            LingoExpr::DeleteChunk(chunk) => if let LingoExpr::ChunkExpr(kind,index,end,source)=*chunk {
                let source_expr = *source;
                self.frames.push(EvalFrame::ApplyDeleteChunk { chunk_type: kind, has_end:end.is_some(), source:source_expr.clone() });
                self.frames.push(EvalFrame::ReadChunkSource(source_expr));
                if let Some(end)=end { self.frames.push(EvalFrame::ConvertEnd); self.frames.push(EvalFrame::Evaluate(*end)); }
                self.frames.push(EvalFrame::ConvertIndex);
                self.frames.push(EvalFrame::Evaluate(*index));
            } else { return Some(EvalTurn::Complete(Err(ScriptError::new("Expected chunk expression after delete".to_owned())))); },
            LingoExpr::ThePropOf(object,name) => {
                let compound = name == "number" || name == "count";
                match (*object, compound) {
                    (LingoExpr::ObjProp(inner_object, inner_property), true) => {
                        self.frames.push(EvalFrame::ApplyObjProp(format!("{} of {}", name, inner_property)));
                        self.frames.push(EvalFrame::Evaluate(*inner_object));
                    }
                    (object, _) => {
                        self.frames.push(EvalFrame::ApplyObjProp(name));
                        self.frames.push(EvalFrame::Evaluate(object));
                    }
                }
            }
            LingoExpr::RectLiteral(values) => {
                if values.len()!=1 { return Some(EvalTurn::Complete(Err(ScriptError::new("RectLiteral must have 1 tuple of 4 elements".to_owned())))); }
                let (a,b,c,d)=values.into_iter().next().unwrap(); self.frames.push(EvalFrame::ApplyRect);
                self.frames.extend([EvalFrame::Evaluate(d),EvalFrame::Evaluate(c),EvalFrame::Evaluate(b),EvalFrame::Evaluate(a)]);
            }
            LingoExpr::PointLiteral(values) => {
                if values.len()!=1 { return Some(EvalTurn::Complete(Err(ScriptError::new("PointLiteral must have 1 tuple of 2 elements".to_owned())))); }
                let (a,b)=values.into_iter().next().unwrap(); self.frames.push(EvalFrame::ApplyPoint);
                self.frames.extend([EvalFrame::Evaluate(b),EvalFrame::Evaluate(a)]);
            }
            LingoExpr::MemberRef(member,cast) => {
                let has_cast=cast.is_some(); self.frames.push(EvalFrame::ApplyMember{has_cast});
                if let Some(cast)=cast { self.frames.push(EvalFrame::Evaluate(*cast)); }
                self.frames.push(EvalFrame::Evaluate(*member));
            }
            LingoExpr::IfThen(cond,body) => { self.frames.push(EvalFrame::ApplyIf{body:*body}); self.frames.push(EvalFrame::Evaluate(*cond)); }
            LingoExpr::Identifier(name) => {
                let result=session.with_player(self.player_id, |context| get_eval_top_level_prop(context.player,context.symbols,&name)).ok_or_else(crate::player::cancelled_scope_error).and_then(|result| result);
                let value = eval_turn_try!(result);
                self.values.push(eval_turn_try!(Self::validate_ref(session, self.player_id, &value)));
            }
        }
        None
    }

    fn push_binary(&mut self, op: EvalBinary, left: LingoExpr, right: LingoExpr) {
        self.frames.push(EvalFrame::ApplyBinary(op));
        self.frames.push(EvalFrame::Evaluate(right));
        self.frames.push(EvalFrame::Evaluate(left));
    }
    fn push_put(&mut self, kind:u8, value:LingoExpr, target:LingoExpr) {
        self.frames.push(EvalFrame::ApplyPut{kind,target});
        self.frames.push(EvalFrame::Evaluate(value));
    }

    fn set_indexed_value(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        list: &DatumRef,
        index: &DatumRef,
        value: &DatumRef,
    ) -> Result<DatumRef, ScriptError> {
        let index_datum = player.get_datum(index).clone();
        match player.get_datum(list) {
            Datum::List(..) => {
                let i = index_datum.int_value()?;
                let i = if i >= 1 { (i - 1) as usize } else { 0 };
                let (_, items, _) = player.get_datum_mut(list).to_list_mut()?;
                if i >= items.len() { return Err(ScriptError::new(format!("List index {} out of bounds (list has {} items)", i + 1, items.len()))); }
                items[i] = value.clone();
            }
            Datum::PropList(..) => PropListUtils::set_at(player, symbols, list, index, value)?,
            Datum::Point(..) => {
                let i = index_datum.int_value()?;
                let i = if i >= 1 { (i - 1) as usize } else { 0 };
                if i >= 2 { return Err(ScriptError::new(format!("Point index {} out of bounds", i + 1))); }
                let (val, is_float) = Datum::datum_to_inline_component(player.get_datum(value))?;
                let (vals, flags) = player.get_datum_mut(list).to_point_inline_mut()?;
                vals[i] = val; Datum::inline_set_float(flags, i, is_float);
            }
            Datum::Rect(..) => {
                let i = index_datum.int_value()?;
                let i = if i >= 1 { (i - 1) as usize } else { 0 };
                if i >= 4 { return Err(ScriptError::new(format!("Rect index {} out of bounds", i + 1))); }
                let (val, is_float) = Datum::datum_to_inline_component(player.get_datum(value))?;
                let (vals, flags) = player.get_datum_mut(list).to_rect_inline_mut()?;
                vals[i] = val; Datum::inline_set_float(flags, i, is_float);
            }
            datum => return Err(ScriptError::new(format!("Cannot assign to index of type: {}", datum.type_str()))),
        }
        Ok(value.clone())
    }

    fn get_indexed_value(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        list: &DatumRef,
        index: &DatumRef,
    ) -> Result<DatumRef, ScriptError> {
        match player.get_datum(list) {
            Datum::List(_, items, _) => {
                let i = player.get_datum(index).int_value()?;
                if i < 1 || i as usize > items.len() {
                    Err(ScriptError::new(format!("List index {} out of bounds (list has {} items)", i, items.len())))
                } else { Ok(items[(i - 1) as usize].clone()) }
            }
            Datum::PropList(pairs, sorted) => PropListUtils::get_at(pairs, index, &player.allocator, symbols, *sorted),
            Datum::Point(vals, flags) => {
                let i = player.get_datum(index).int_value()?;
                if !(1..=2).contains(&i) { return Err(ScriptError::new(format!("Point index {} out of bounds (must be 1 or 2)", i))); }
                let i = (i - 1) as usize;
                Ok(player.alloc_datum(Datum::inline_component_to_datum(vals[i], Datum::inline_is_float(*flags, i))))
            }
            Datum::Rect(vals, flags) => {
                let i = player.get_datum(index).int_value()?;
                if !(1..=4).contains(&i) { return Err(ScriptError::new(format!("Rect index {} out of bounds (must be 1-4)", i))); }
                let i = (i - 1) as usize;
                Ok(player.alloc_datum(Datum::inline_component_to_datum(vals[i], Datum::inline_is_float(*flags, i))))
            }
            Datum::String(value) => {
                let i = player.get_datum(index).int_value()?;
                if i < 1 || i as usize > value.chars().count() { Ok(player.alloc_datum(Datum::String(String::new()))) }
                else { Ok(player.alloc_datum(Datum::String(value.chars().nth((i - 1) as usize).unwrap().to_string()))) }
            }
            Datum::SpriteRef(sprite_number) => {
                let prop_name = match player.get_datum(index) {
                    Datum::Symbol(name) => symbols.display(name).map_err(|_| ScriptError::new_code(crate::player::ScriptErrorCode::InvalidReference, "foreign symbol".to_owned()))?.to_owned(),
                    Datum::String(name) => name.clone(),
                    other => return Err(ScriptError::new(format!("Cannot index sprite {} with {}", sprite_number, other.type_str()))),
                };
                let prop_symbol = symbols.intern(&prop_name);
                let result = crate::player::score::sprite_get_prop(player, symbols, *sprite_number, prop_symbol)?;
                Ok(player.last_sprite_prop_ref.take().unwrap_or_else(|| player.alloc_datum(result)))
            }
            datum => Err(ScriptError::new(format!("Cannot index non-list type: {:?}", datum.type_enum()))),
        }
    }

    fn schedule_chunk_fallback(&mut self, target: LingoExpr) {
        if let LingoExpr::ListAccess(list, index) = target {
            self.frames.push(EvalFrame::ApplyListAccess);
            self.frames.push(EvalFrame::Evaluate(*index));
            self.frames.push(EvalFrame::Evaluate(*list));
        } else {
            self.frames.push(EvalFrame::Evaluate(target));
        }
    }

    fn apply_frame(&mut self, session:&mut crate::player::session::RuntimeSession, frame:EvalFrame) -> Option<EvalTurn> {
        match frame {
            EvalFrame::Evaluate(expr) => self.frames.push(EvalFrame::Evaluate(expr)),
            EvalFrame::ApplyList(count) => {
                let items=match self.pop_values(count){Ok(v)=>v,Err(e)=>return Some(EvalTurn::Complete(Err(e)))};
                let result=session.with_player(self.player_id,|context| Ok::<_,ScriptError>(context.player.alloc_datum(Datum::List(DatumType::List,items.into_iter().collect(),false)))).ok_or_else(crate::player::cancelled_scope_error).and_then(|result| result);
                self.values.push(eval_turn_try!(result));
            }
            EvalFrame::ApplyPropList(count) => {
                let vals=match self.pop_values(count*2){Ok(v)=>v,Err(e)=>return Some(EvalTurn::Complete(Err(e)))};
                let pairs=vals.chunks_exact(2).map(|v|(v[0].clone(),v[1].clone())).collect();
                let result=session.with_player(self.player_id,|context| Ok::<_,ScriptError>(context.player.alloc_datum(Datum::PropList(pairs,false)))).ok_or_else(crate::player::cancelled_scope_error).and_then(|result| result);
                self.values.push(eval_turn_try!(result));
            }
            EvalFrame::ApplyNot => { let v=eval_turn_try!(self.pop_value()); let result=session.with_player(self.player_id,|context| {let n=context.player.get_datum(&v).int_value()?; Ok::<_,ScriptError>(context.player.alloc_datum(Datum::Int(i32::from(n==0))))}).ok_or_else(crate::player::cancelled_scope_error).and_then(|result| result); self.values.push(eval_turn_try!(result)); }
            EvalFrame::ApplyBinary(op) => {
                let args=match self.pop_values(2){Ok(v)=>v,Err(e)=>return Some(EvalTurn::Complete(Err(e)))}; let l=args[0].clone();let r=args[1].clone();
                let result=session.with_player(self.player_id,|context| {
                    let p=context.player; let ld=p.get_datum(&l);let rd=p.get_datum(&r);
                    let d=match op { EvalBinary::Add=>add_datums(ld.clone(),rd.clone(),p,context.symbols)?, EvalBinary::Subtract=>subtract_datums(ld.clone(),rd.clone(),p,context.symbols)?, EvalBinary::Multiply=>multiply_datums(l.clone(),r.clone(),p,context.symbols)?, EvalBinary::Divide=>divide_datums(l.clone(),r.clone(),p,context.symbols)?, EvalBinary::Modulo=>crate::player::bytecode::arithmetics::ArithmeticsBytecodeHandler::modulo_datums(&l,&r,p,context.symbols)?, EvalBinary::Join(pad)=>return StringBytecodeHandler::concat_datums(l,r,p,context.symbols,pad), EvalBinary::Eq=>Datum::Int(i32::from(crate::player::compare::datum_equals(ld,rd,&p.allocator,context.symbols)?)), EvalBinary::Ne=>Datum::Int(i32::from(!crate::player::compare::datum_equals(ld,rd,&p.allocator,context.symbols)?)), EvalBinary::Lt=>Datum::Int(i32::from(crate::player::compare::datum_less_than(ld,rd,&p.allocator,context.symbols)?)), EvalBinary::Gt=>Datum::Int(i32::from(crate::player::compare::datum_greater_than(ld,rd,&p.allocator,context.symbols)?)), EvalBinary::Le=>Datum::Int(i32::from({let e=crate::player::compare::datum_equals(ld,rd,&p.allocator,context.symbols)?;let l=crate::player::compare::datum_less_than(ld,rd,&p.allocator,context.symbols)?;e||l})), EvalBinary::Ge=>Datum::Int(i32::from({let e=crate::player::compare::datum_equals(ld,rd,&p.allocator,context.symbols)?;let g=crate::player::compare::datum_greater_than(ld,rd,&p.allocator,context.symbols)?;e||g})), EvalBinary::And=>{let left=ld.int_value()?;let right=rd.int_value()?;Datum::Int(i32::from(left!=0 && right!=0))}, EvalBinary::Or=>{let left=ld.int_value()?;let right=rd.int_value()?;Datum::Int(i32::from(left!=0 || right!=0))}}; Ok::<_,ScriptError>(p.alloc_datum(d))
                }).ok_or_else(crate::player::cancelled_scope_error).and_then(|result| result);
                self.values.push(eval_turn_try!(result));
            }
            EvalFrame::ApplyObjProp(name) => {
                let object = eval_turn_try!(self.pop_value());
                let flash_request = session.with_player(self.player_id, |context| {
                    let value = crate::player::driver::checked_internal_datum(
                        context.player,
                        context.symbols,
                        &object,
                    )?.clone();
                    if matches!(value, Datum::FlashObjectRef(_)) {
                        let symbol = context.symbols.intern(&name);
                        Ok::<_, ScriptError>(Some(
                            crate::player::handlers::datum_handlers::flash_object::FlashObjectDatumHandlers::prepare_get_prop(
                                context.player,
                                &object,
                                &context
                                    .symbols
                                    .display(&symbol)
                                    .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
                                    .to_owned(),
                            )?,
                        ))
                    } else {
                        Ok(None)
                    }
                }).ok_or_else(crate::player::cancelled_scope_error).and_then(|result| result);
                let flash_request = eval_turn_try!(flash_request);
                if let Some(request) = flash_request {
                    let capability = self.new_action(false);
                    return Some(EvalTurn::Pending {
                        request: EvalPending::Object {
                            capability,
                            request: crate::player::driver::InternalVmRequest::Flash(request),
                            reason: None,
                        },
                    });
                }
                let result = session.with_player(self.player_id, |context| {
                    let symbol = context.symbols.intern(&name);
                    get_obj_prop(context.player, context.symbols, &object, symbol)
                }).ok_or_else(crate::player::cancelled_scope_error).and_then(|r| r);
                let result = eval_turn_try!(result);
                self.values.push(eval_turn_try!(Self::validate_ref(session, self.player_id, &result)));
            }
            EvalFrame::ApplyHandler{name,argc} => {
                let args=match self.pop_values(argc){Ok(v)=>v,Err(e)=>return Some(EvalTurn::Complete(Err(e)))}; let symbol=match Self::intern(session,self.player_id,&name){Ok(s)=>s,Err(e)=>return Some(EvalTurn::Complete(Err(e)))};
                let receiver = match session.with_player(self.player_id, |context| {
                    if context.player.current_breakpoint.is_none() || context.player.scope_count == 0 {
                        return Ok::<_, ScriptError>(None);
                    }
                    let scope_idx = context.player.eval_scope_index
                        .unwrap_or(context.player.scope_count - 1) as usize;
                    let Some(scope) = context.player.scopes.get(scope_idx) else { return Ok(None); };
                    let Some(receiver_ref) = scope.receiver.clone() else { return Ok(None); };
                    let script_ref = scope.script_ref.clone();
                    let Some(script) = context.player.movie.cast_manager.get_script_by_ref(&script_ref) else { return Ok(None); };
                    if script.get_own_handler(symbol.clone()).is_some() {
                        Ok(Some(context.player.alloc_datum(Datum::ScriptInstanceRef(receiver_ref))))
                    } else { Ok(None) }
                }).ok_or_else(crate::player::cancelled_scope_error).and_then(|result| result) {
                    Ok(value) => value,
                    Err(error) => return Some(EvalTurn::Complete(Err(error))),
                };
                if let Some(receiver) = receiver {
                    let capability = self.new_action(false);
                    return Some(EvalTurn::Pending { request: EvalPending::Object {
                        capability,
                        request: crate::player::driver::InternalVmRequest::Object { receiver, name: symbol.clone(), args },
                        reason: None,
                    }});
                }
                match session.dispatch_global(self.player_id,&symbol,&args) {
                    Ok(crate::player::driver::GlobalDispatch::SyncResult(result)) => {
                        let result = eval_turn_try!(result);
                        self.values.push(eval_turn_try!(Self::validate_ref(session, self.player_id, &result)));
                    },
                    Ok(crate::player::driver::GlobalDispatch::Child { receiver, handler_ref }) => {
                        let capability = self.new_action(false);
                        return Some(EvalTurn::Pending { request: EvalPending::Global {
                            capability,
                            request: crate::player::driver::InternalVmRequest::Global { name: symbol, args: args.clone() },
                            reason: None,
                            prepared_child: Some(crate::player::eval::PreparedGlobal::Child {
                                receiver,
                                handler_ref,
                                args: args.clone(),
                                use_raw_arg_list: true,
                                completion: None,
                            }),
                        }});
                    }
                    Ok(crate::player::driver::GlobalDispatch::ChildPrepared { receiver, handler_ref, args: prepared_args }) => {
                        let capability = self.new_action(false);
                        return Some(EvalTurn::Pending { request: EvalPending::Global {
                            capability,
                            request: crate::player::driver::InternalVmRequest::Global { name: symbol, args: args.clone() },
                            reason: None,
                            prepared_child: Some(crate::player::eval::PreparedGlobal::Child {
                                receiver,
                                handler_ref,
                                args: prepared_args,
                                use_raw_arg_list: false,
                                completion: None,
                            }),
                        }});
                    }
                    Ok(crate::player::driver::GlobalDispatch::ChildWithCompletion {
                        receiver,
                        handler_ref,
                        args: prepared_args,
                        completion,
                    }) => {
                        let capability = self.new_action(false);
                        return Some(EvalTurn::Pending { request: EvalPending::Global {
                            capability,
                            request: crate::player::driver::InternalVmRequest::Global { name: symbol, args },
                            reason: None,
                            prepared_child: Some(crate::player::eval::PreparedGlobal::Child {
                                receiver,
                                handler_ref,
                                args: prepared_args,
                                use_raw_arg_list: false,
                                completion: Some(completion),
                            }),
                        }});
                    }
                    Ok(crate::player::driver::GlobalDispatch::AncestorChildren { calls }) => {
                        let capability = self.new_action(false);
                        return Some(EvalTurn::Pending { request: EvalPending::Global {
                            capability,
                            request: crate::player::driver::InternalVmRequest::Global { name: symbol, args },
                            reason: None,
                            prepared_child: Some(crate::player::eval::PreparedGlobal::Ancestors { calls }),
                        }});
                    }
                    Ok(crate::player::driver::GlobalDispatch::ChildSequence { plan }) => {
                        let capability = self.new_action(false);
                        return Some(EvalTurn::Pending { request: EvalPending::Global {
                            capability,
                            request: crate::player::driver::InternalVmRequest::Global { name: symbol, args },
                            reason: None,
                            prepared_child: Some(crate::player::eval::PreparedGlobal::Broadcast { plan }),
                        }});
                    }
                    Ok(crate::player::driver::GlobalDispatch::PendingRequest { request, reason }) => {
                        let capability = self.new_action(false);
                        return Some(EvalTurn::Pending { request: EvalPending::Global {
                            capability,
                            request,
                            reason: Some(reason),
                            prepared_child: None,
                        }});
                    }
                    Ok(crate::player::driver::GlobalDispatch::Pending { reason }) => {
                        let capability = self.new_action(false);
                        return Some(EvalTurn::Pending { request: EvalPending::Global {
                            capability,
                            request: crate::player::driver::InternalVmRequest::Global { name: symbol, args },
                            reason: Some(reason),
                            prepared_child: None,
                        }});
                    }
                    Err(e) => return Some(EvalTurn::Complete(Err(e))),
                }
            }
            EvalFrame::ApplyObjHandler{name,argc} => {
                let args=match self.pop_values(argc){Ok(v)=>v,Err(e)=>return Some(EvalTurn::Complete(Err(e)))}; let receiver=match self.pop_value(){Ok(v)=>v,Err(e)=>return Some(EvalTurn::Complete(Err(e)))}; let symbol=match Self::intern(session,self.player_id,&name){Ok(s)=>s,Err(e)=>return Some(EvalTurn::Complete(Err(e)))};
                let capability = self.new_action(false);
                return Some(EvalTurn::Pending{request:EvalPending::Object{capability,request:crate::player::driver::InternalVmRequest::Object{receiver,name:symbol,args},reason:None}});
            }
            EvalFrame::ApplyListAccess => {
                let index=eval_turn_try!(self.pop_value());let list=eval_turn_try!(self.pop_value());
                eval_turn_try!(Self::validate_ref(session, self.player_id, &index));
                eval_turn_try!(Self::validate_ref(session, self.player_id, &list));
                let result = session.with_player(self.player_id, |context| {
                    let p = context.player;
                    match p.get_datum(&list) {
                        Datum::List(_, items, _) => {
                            let i = p.get_datum(&index).int_value()?;
                            if i < 1 || i as usize > items.len() {
                                Err(ScriptError::new(format!("List index {} out of bounds (list has {} items)", i, items.len())))
                            } else {
                                Ok(items[(i - 1) as usize].clone())
                            }
                        }
                        Datum::PropList(pairs, sorted) => PropListUtils::get_at(
                            pairs, &index, &p.allocator, context.symbols, *sorted,
                        ),
                        Datum::Point(vals, flags) => {
                            let i = p.get_datum(&index).int_value()?;
                            if !(1..=2).contains(&i) { return Err(ScriptError::new(format!("Point index {} out of bounds (must be 1 or 2)", i))); }
                            let i = (i - 1) as usize;
                            Ok(p.alloc_datum(Datum::inline_component_to_datum(vals[i], Datum::inline_is_float(*flags, i))))
                        }
                        Datum::Rect(vals, flags) => {
                            let i = p.get_datum(&index).int_value()?;
                            if !(1..=4).contains(&i) { return Err(ScriptError::new(format!("Rect index {} out of bounds (must be 1-4)", i))); }
                            let i = (i - 1) as usize;
                            Ok(p.alloc_datum(Datum::inline_component_to_datum(vals[i], Datum::inline_is_float(*flags, i))))
                        }
                        Datum::String(value) => {
                            let i = p.get_datum(&index).int_value()?;
                            if i < 1 || i as usize > value.chars().count() { Ok(p.alloc_datum(Datum::String(String::new()))) }
                            else { Ok(p.alloc_datum(Datum::String(value.chars().nth((i - 1) as usize).unwrap().to_string()))) }
                        }
                        Datum::SpriteRef(sprite_number) => {
                            let prop_name = match p.get_datum(&index) {
                                Datum::Symbol(name) => context.symbols.display(name).map_err(|_| ScriptError::new_code(crate::player::ScriptErrorCode::InvalidReference, "foreign symbol".to_owned()))?.to_owned(),
                                Datum::String(name) => name.clone(),
                                other => return Err(ScriptError::new(format!("Cannot index sprite {} with {}", sprite_number, other.type_str()))),
                            };
                            let prop_symbol = context.symbols.intern(&prop_name);
                            let result = crate::player::score::sprite_get_prop(p, context.symbols, *sprite_number, prop_symbol)?;
                            Ok(p.last_sprite_prop_ref.take().unwrap_or_else(|| p.alloc_datum(result)))
                        }
                        _ => Err(ScriptError::new("cannot index value".to_owned())),
                    }
                }).ok_or_else(crate::player::cancelled_scope_error).and_then(|result| result);
                let result = eval_turn_try!(result);
                self.values.push(eval_turn_try!(Self::validate_ref(session, self.player_id, &result)));
            }
            EvalFrame::ApplyChunkAccess { property: prop_name, target } => {
                let index = eval_turn_try!(self.pop_value());
                let object = eval_turn_try!(self.pop_value());
                eval_turn_try!(Self::validate_ref(session, self.player_id, &index));
                eval_turn_try!(Self::validate_ref(session, self.player_id, &object));
                let result = session.with_player(self.player_id, |context| {
                    let p = context.player;
                    let i = p.get_datum(&index).int_value()?;
                    let prop_symbol = context.symbols.intern(&prop_name);
                    match p.get_datum(&object).clone() {
                        Datum::String(_) => Ok(Some(p.alloc_datum(
                            crate::player::handlers::datum_handlers::string::StringDatumUtils::get_prop_ref(
                                p, context.symbols, &object, prop_symbol, i, i,
                            )?,
                        ))),
                        Datum::CastMember(member_ref) => {
                            let Some(text) = p.movie.cast_manager.find_member_by_ref(&member_ref)
                                .and_then(|member| match &member.member_type {
                                    crate::player::cast_member::CastMemberType::Field(field) => Some(field.text.clone()),
                                    crate::player::cast_member::CastMemberType::Text(text) => Some(text.text.clone()),
                                    _ => None,
                                }) else { return Ok(None) };
                            let chunk_type = StringChunkType::from_symbol(&prop_symbol, context.symbols)?;
                            let chunk = StringChunkExpr { chunk_type, start: i, end: i, item_delimiter: p.movie.item_delimiter };
                            let resolved = StringChunkUtils::resolve_chunk_expr_string(&text, &chunk)?;
                            Ok(Some(p.alloc_datum(Datum::StringChunk(
                                crate::director::lingo::datum::StringChunkSource::Member(member_ref), chunk, resolved,
                            ))))
                        }
                        Datum::StringChunk(..) => {
                            let text = p.get_datum(&object).string_value(context.symbols)?;
                            let chunk_type = StringChunkType::from_symbol(&prop_symbol, context.symbols)?;
                            let chunk = StringChunkExpr { chunk_type, start: i, end: i, item_delimiter: p.movie.item_delimiter };
                            let resolved = StringChunkUtils::resolve_chunk_expr_string(&text, &chunk)?;
                            Ok(Some(p.alloc_datum(Datum::StringChunk(
                                crate::director::lingo::datum::StringChunkSource::Datum(object), chunk, resolved,
                            ))))
                        }
                        _ => Ok(None),
                    }
                }).ok_or_else(crate::player::cancelled_scope_error).and_then(|r| r);
                let result = eval_turn_try!(result);
                if let Some(value) = result {
                    self.values.push(eval_turn_try!(Self::validate_ref(session, self.player_id, &value)));
                } else {
                    self.schedule_chunk_fallback(target);
                }
            }
            EvalFrame::ApplyAssignment(target) => { let value=eval_turn_try!(self.pop_value()); match target {LingoExpr::Identifier(name)=>{let result=session.with_player(self.player_id,|context|{let p=context.player;if name.starts_with("the "){let sym=context.symbols.intern(&name[4..]);p.set_movie_prop(context.symbols,sym,p.get_datum(&value).clone())?;}else if let Some(receiver)=current_do_receiver_with_prop(p,context.symbols,&name)?{let sym=context.symbols.intern(&name);script_set_prop(p,context.symbols,&receiver,sym,&value,true)?;}else{let sym=context.symbols.intern(&name);p.globals.insert(sym,value.clone());}Ok::<_,ScriptError>(value)}).ok_or_else(crate::player::cancelled_scope_error).and_then(|result| result);self.values.push(eval_turn_try!(result));},LingoExpr::ObjProp(object,name)|LingoExpr::ThePropOf(object,name)=>{self.frames.push(EvalFrame::ApplySetProperty(name));self.frames.push(EvalFrame::Evaluate(*object));self.values.push(value);},LingoExpr::ListAccess(list,index)=>{
                            if let LingoExpr::ObjProp(object, property) = list.as_ref() {
                                let target = LingoExpr::ListAccess(
                                    Box::new(LingoExpr::ObjProp(object.clone(), property.clone())),
                                    index.clone(),
                                );
                                self.frames.push(EvalFrame::ProbeIndexedAssignment { value, target });
                                self.frames.push(EvalFrame::Evaluate((**object).clone()));
                            } else {
                                self.frames.push(EvalFrame::ApplyIndexedAssignment { value, property: None });
                                self.frames.push(EvalFrame::Evaluate(*index));
                                self.frames.push(EvalFrame::Evaluate(*list));
                            }
                        },_=>return Some(EvalTurn::Complete(Err(ScriptError::new("invalid assignment target".to_owned()))))} }
            EvalFrame::ProbeIndexedAssignment { value, target } => {
                let object = eval_turn_try!(self.pop_value());
                eval_turn_try!(Self::validate_ref(session, self.player_id, &object));
                let LingoExpr::ListAccess(list, index) = target else {
                    return Some(EvalTurn::Complete(Err(ScriptError::new("invalid indexed assignment probe".to_owned()))));
                };
                let LingoExpr::ObjProp(_, property) = list.as_ref() else {
                    return Some(EvalTurn::Complete(Err(ScriptError::new("invalid indexed assignment probe".to_owned()))));
                };
                let is_s3d = eval_turn_try!(session.with_player(self.player_id, |context| {
                    Ok::<_, ScriptError>(matches!(context.player.get_datum(&object), Datum::Shockwave3dObjectRef(_)))
                }).ok_or_else(crate::player::cancelled_scope_error).and_then(|r| r));
                if is_s3d {
                    self.values.push(object);
                    self.frames.push(EvalFrame::ApplyIndexedAssignment { value, property: Some(property.clone()) });
                    self.frames.push(EvalFrame::Evaluate(*index));
                } else {
                    self.frames.push(EvalFrame::ApplyIndexedAssignment { value, property: None });
                    self.frames.push(EvalFrame::Evaluate(*index));
                    self.frames.push(EvalFrame::Evaluate(*list));
                }
            }
            EvalFrame::ApplyIndexedAssignment { value, property } => {
                let index = eval_turn_try!(self.pop_value());
                let container = eval_turn_try!(self.pop_value());
                eval_turn_try!(Self::validate_ref(session, self.player_id, &index));
                eval_turn_try!(Self::validate_ref(session, self.player_id, &container));
                eval_turn_try!(Self::validate_ref(session, self.player_id, &value));
                if let Some(property) = property {
                    let is_s3d = session.with_player(self.player_id, |context| {
                        Ok::<_, ScriptError>(matches!(context.player.get_datum(&container), Datum::Shockwave3dObjectRef(_)))
                    }).ok_or_else(crate::player::cancelled_scope_error).and_then(|r| r);
                    if eval_turn_try!(is_s3d) {
                        let (property_ref, set_at) = match session.with_player(self.player_id, |context| {
                            let property_ref = context.player.alloc_datum(Datum::Symbol(context.symbols.intern(&property)));
                            Ok::<_, ScriptError>((property_ref, Symbol::builtin(BuiltInSymbol::SetAt)))
                        }).ok_or_else(crate::player::cancelled_scope_error).and_then(|r| r) {
                            Ok(v) => v,
                            Err(error) => return Some(EvalTurn::Complete(Err(error))),
                        };
                        self.frames.push(EvalFrame::ApplySetAtResult(value.clone()));
                        let capability = self.new_action(false);
                        return Some(EvalTurn::Pending { request: EvalPending::Object {
                            capability,
                            request: crate::player::driver::InternalVmRequest::Object {
                                receiver: container,
                                name: set_at,
                                args: vec![property_ref, index, value.clone()],
                            },
                            reason: None,
                        }});
                    }
                    let result = session.with_player(self.player_id, |context| {
                        let symbol = context.symbols.intern(&property);
                        get_obj_prop(context.player, context.symbols, &container, symbol)
                    }).ok_or_else(crate::player::cancelled_scope_error).and_then(|r| r);
                    let list = eval_turn_try!(result);
                    let result = session.with_player(self.player_id, |context| {
                        Self::set_indexed_value(context.player, context.symbols, &list, &index, &value)
                    }).ok_or_else(crate::player::cancelled_scope_error).and_then(|r| r);
                    self.values.push(eval_turn_try!(result));
                } else {
                    let result = session.with_player(self.player_id, |context| {
                        Self::set_indexed_value(context.player, context.symbols, &container, &index, &value)
                    }).ok_or_else(crate::player::cancelled_scope_error).and_then(|r| r);
                    self.values.push(eval_turn_try!(result));
                }
            }
            EvalFrame::ApplySetAtResult(value) => {
                let _ = eval_turn_try!(self.pop_value());
                self.values.push(value);
            }
            EvalFrame::ApplyPut{kind,target}=>{
                let value=eval_turn_try!(self.pop_value());
                if kind==3 {
                    let result = session.with_player(self.player_id, |mut context| {
                        crate::player::handlers::manager::BuiltInHandlerManager::put(&mut context, &vec![value])
                    }).ok_or_else(crate::player::cancelled_scope_error).and_then(|result| result);
                    self.values.push(eval_turn_try!(result));
                } else if let LingoExpr::ChunkExpr(chunk_type,index,end,source)=target {
                    let source_expr=*source;
                    let apply_target=LingoExpr::ChunkExpr(
                        chunk_type.clone(), index.clone(), end.clone(),
                        Box::new(source_expr.clone()),
                    );
                    self.frames.push(EvalFrame::ApplyPutChunk{kind,target:apply_target,value:value.clone()});
                    self.frames.push(EvalFrame::ReadChunkSource(source_expr));
                    self.frames.push(EvalFrame::FormatPutValue(value));
                    if let Some(end)=end { self.frames.push(EvalFrame::ConvertEnd); self.frames.push(EvalFrame::Evaluate(*end)); }
                    self.frames.push(EvalFrame::ConvertIndex);
                    self.frames.push(EvalFrame::Evaluate(*index));
                } else if let LingoExpr::Identifier(name)=target {
                    let result=session.with_player(self.player_id,|context|{
                        let sym=context.symbols.intern(&name);
                        if kind == 0 {
                            context.player.globals.insert(sym, value.clone());
                        } else {
                            let old=context.player.globals.get(&sym).cloned().unwrap_or_else(||context.player.alloc_datum(Datum::String(String::new())));
                            use crate::player::datum_formatting::datum_to_string_for_concat;
                            let text = if kind == 1 {
                                let value_text = datum_to_string_for_concat(context.player.get_datum(&value),context.symbols,context.player)?;
                                let old_text = datum_to_string_for_concat(context.player.get_datum(&old),context.symbols,context.player)?;
                                format!("{}{}", value_text, old_text)
                            } else {
                                let old_text = datum_to_string_for_concat(context.player.get_datum(&old),context.symbols,context.player)?;
                                let value_text = datum_to_string_for_concat(context.player.get_datum(&value),context.symbols,context.player)?;
                                format!("{}{}", old_text, value_text)
                            };
                            let result_ref = context.player.alloc_datum(Datum::String(text));
                            context.player.globals.insert(sym, result_ref);
                        }
                        Ok::<_,ScriptError>(DatumRef::Void)
                    }).ok_or_else(crate::player::cancelled_scope_error).and_then(|result| result);self.values.push(eval_turn_try!(result));
                } else { return Some(EvalTurn::Complete(Err(ScriptError::new("invalid put target".to_owned())))); }
            }
            EvalFrame::FormatPutValue(value) => {
                eval_turn_try!(Self::validate_ref(session, self.player_id, &value));
                let result = session.with_player(self.player_id, |context| {
                    use crate::player::datum_formatting::datum_to_string_for_concat;
                    let text = datum_to_string_for_concat(context.player.get_datum(&value), context.symbols, context.player)?;
                    Ok::<_, ScriptError>(context.player.alloc_datum(Datum::String(text)))
                }).ok_or_else(crate::player::cancelled_scope_error).and_then(|r| r);
                self.values.push(eval_turn_try!(result));
            }
            EvalFrame::ReadChunkSource(source) => {
                match source {
                    LingoExpr::Identifier(name) => {
                        let result = session.with_player(self.player_id, |context| {
                            let symbol = context.symbols.intern(&name);
                            Ok::<_, ScriptError>(context.player.globals.get(&symbol).cloned().unwrap_or_else(|| {
                                context.player.alloc_datum(Datum::String(String::new()))
                            }))
                        }).ok_or_else(crate::player::cancelled_scope_error).and_then(|r| r);
                        let value = eval_turn_try!(result);
                        self.values.push(eval_turn_try!(Self::validate_ref(session, self.player_id, &value)));
                    }
                    expression => {
                        self.frames.push(EvalFrame::ReadChunkSourceDone);
                        self.frames.push(EvalFrame::Evaluate(expression));
                    }
                }
            }
            EvalFrame::ReadChunkSourceDone => {
                let value = eval_turn_try!(self.pop_value());
                self.values.push(eval_turn_try!(Self::validate_ref(session, self.player_id, &value)));
            }
            EvalFrame::ConvertIndex => {
                let input = eval_turn_try!(self.pop_value());
                eval_turn_try!(Self::validate_ref(session, self.player_id, &input));
                let result = session.with_player(self.player_id, |context| {
                    let index = context.player.get_datum(&input).int_value()?;
                    Ok::<_, ScriptError>(context.player.alloc_datum(Datum::Int(index)))
                }).ok_or_else(crate::player::cancelled_scope_error).and_then(|r| r);
                self.values.push(eval_turn_try!(result));
            }
            EvalFrame::ConvertEnd => {
                let input = eval_turn_try!(self.pop_value());
                eval_turn_try!(Self::validate_ref(session, self.player_id, &input));
                let result = session.with_player(self.player_id, |context| {
                    let end = context.player.get_datum(&input).int_value()?;
                    Ok::<_, ScriptError>(context.player.alloc_datum(Datum::Int(end)))
                }).ok_or_else(crate::player::cancelled_scope_error).and_then(|r| r);
                self.values.push(eval_turn_try!(result));
            }
            EvalFrame::ApplyPutChunk{kind,target,value}=>{
                let has_end = matches!(&target, LingoExpr::ChunkExpr(_, _, Some(_), _));
                let source_ref=eval_turn_try!(self.pop_value());
                let formatted_value=eval_turn_try!(self.pop_value());
                let end=if has_end { eval_turn_try!(self.pop_value()) } else { DatumRef::Void };
                let index=eval_turn_try!(self.pop_value());
                eval_turn_try!(Self::validate_ref(session, self.player_id, &source_ref));
                eval_turn_try!(Self::validate_ref(session, self.player_id, &value));
                eval_turn_try!(Self::validate_ref(session, self.player_id, &formatted_value));
                eval_turn_try!(Self::validate_ref(session, self.player_id, &index));
                if has_end { eval_turn_try!(Self::validate_ref(session, self.player_id, &end)); }
                let LingoExpr::ChunkExpr(chunk_type,_,_,source_expr)=target else { unreachable!() };
                let result=session.with_player(self.player_id,|context|{let p=context.player;let i=p.get_datum(&index).int_value()?;let e=if !matches!(end,DatumRef::Void){p.get_datum(&end).int_value()?}else{i};let text=p.get_datum(&source_ref).string_value(context.symbols)?;let value_text=p.get_datum(&formatted_value).string_value(context.symbols)?;let chunk=StringChunkExpr{chunk_type:StringChunkType::from_symbol(&chunk_type,context.symbols)?,start:i,end:e,item_delimiter:p.movie.item_delimiter};let changed=match kind{0=>StringChunkUtils::string_by_putting_into_chunk(&text,&chunk,&value_text)?,1=>StringChunkUtils::string_by_putting_before_chunk(&text,&chunk,&value_text)?,_=>StringChunkUtils::string_by_putting_after_chunk(&text,&chunk,&value_text)?};write_chunk_source(p,context.symbols,&source_expr,changed);Ok::<_,ScriptError>(DatumRef::Void)}).ok_or_else(crate::player::cancelled_scope_error).and_then(|result|result);self.values.push(eval_turn_try!(result));
            }
            EvalFrame::ApplyChunkSource => {
                let source = eval_turn_try!(self.pop_value());
                eval_turn_try!(Self::validate_ref(session, self.player_id, &source));
                let result = session.with_player(self.player_id, |context| {
                    let text = context.player.get_datum(&source).string_value(context.symbols)?;
                    Ok::<_, ScriptError>((source, context.player.alloc_datum(Datum::String(text))))
                }).ok_or_else(crate::player::cancelled_scope_error).and_then(|r| r);
                let (source, converted) = eval_turn_try!(result);
                self.values.push(source);
                self.values.push(converted);
            }
            EvalFrame::ApplyChunk{chunk_type,has_end}=>{
                let end=if has_end{eval_turn_try!(self.pop_value())}else{DatumRef::Void};
                let index=eval_turn_try!(self.pop_value());
                let converted=eval_turn_try!(self.pop_value());
                let source=eval_turn_try!(self.pop_value());
                let result=session.with_player(self.player_id,|context|{
                    let p=context.player;
                    let i=p.get_datum(&index).int_value()?;
                    let e=if has_end{p.get_datum(&end).int_value()?}else{i};
                    let text=p.get_datum(&converted).string_value(context.symbols)?;
                    let chunk=StringChunkExpr{chunk_type:StringChunkType::from_symbol(&chunk_type,context.symbols)?,start:i,end:e,item_delimiter:p.movie.item_delimiter};
                    let resolved=StringChunkUtils::resolve_chunk_expr_string(&text,&chunk)?;
                    let datum=match p.get_datum(&source){
                        Datum::CastMember(member_ref)=>Datum::StringChunk(crate::director::lingo::datum::StringChunkSource::Member(*member_ref),chunk,resolved),
                        Datum::StringChunk(..)=>Datum::StringChunk(crate::director::lingo::datum::StringChunkSource::Datum(source.clone()),chunk,resolved),
                        _=>Datum::String(resolved),
                    };
                    Ok::<_,ScriptError>(p.alloc_datum(datum))
                }).ok_or_else(crate::player::cancelled_scope_error).and_then(|r| r);
                let value=eval_turn_try!(result);
                self.values.push(eval_turn_try!(Self::validate_ref(session,self.player_id,&value)));
            }
            EvalFrame::ApplySetProperty(name) => {
                let receiver = match self.pop_value() { Ok(v) => v, Err(e) => return Some(EvalTurn::Complete(Err(e))) };
                let value = match self.pop_value() { Ok(v) => v, Err(e) => return Some(EvalTurn::Complete(Err(e))) };
                eval_turn_try!(Self::validate_ref(session, self.player_id, &receiver));
                eval_turn_try!(Self::validate_ref(session, self.player_id, &value));
                let symbol = match Self::intern(session, self.player_id, &name) { Ok(v) => v, Err(e) => return Some(EvalTurn::Complete(Err(e))) };
                let capability = self.new_action(true);
                return Some(EvalTurn::Pending { request: EvalPending::SetProperty { capability, request: crate::player::driver::InternalVmRequest::SetProperty { receiver, name: symbol, value } } });
            }
            EvalFrame::ApplyRect => {
                let values=match self.pop_values(4){Ok(v)=>v,Err(e)=>return Some(EvalTurn::Complete(Err(e)))};
                let result=session.with_player(self.player_id,|context|{let refs=[&values[0],&values[1],&values[2],&values[3]];let d=Datum::build_rect(context.player.get_datum(refs[0]),context.player.get_datum(refs[1]),context.player.get_datum(refs[2]),context.player.get_datum(refs[3]))?;Ok::<_,ScriptError>(context.player.alloc_datum(d))}).ok_or_else(crate::player::cancelled_scope_error).and_then(|result|result);self.values.push(eval_turn_try!(result));
            }
            EvalFrame::ApplyPoint => {
                let values=match self.pop_values(2){Ok(v)=>v,Err(e)=>return Some(EvalTurn::Complete(Err(e)))};
                let result=session.with_player(self.player_id,|context|{let d=Datum::build_point(context.player.get_datum(&values[0]),context.player.get_datum(&values[1]))?;Ok::<_,ScriptError>(context.player.alloc_datum(d))}).ok_or_else(crate::player::cancelled_scope_error).and_then(|result|result);self.values.push(eval_turn_try!(result));
            }
            EvalFrame::ApplyMember{has_cast} => {
                let cast = if has_cast { Some(eval_turn_try!(self.pop_value())) } else { None };
                let member = eval_turn_try!(self.pop_value());
                let result = session.with_player(self.player_id, |context| {
                    let p = context.player;
                    let member_datum = p.get_datum(&member).clone();
                    let cast_datum = cast.as_ref().map(|r| p.get_datum(r).clone());
                    let found = p.movie.cast_manager.find_member_ref_by_identifiers(
                        context.symbols, &member_datum, cast_datum.as_ref(), &p.allocator,
                    )?;
                    let member_ref = match found {
                        Some(r) => r,
                        None => if let Some(cast_datum) = cast_datum {
                            let cast_lib = match cast_datum {
                                Datum::Int(number) => number,
                                Datum::CastLib(number) => number as i32,
                                Datum::String(name) => p.movie.cast_manager.get_cast_by_name(&name)
                                    .map(|c| c.number as i32).unwrap_or(0),
                                datum => return Err(ScriptError::new(format!(
                                    "Expected int, string, or castLib, got {:?}", datum.type_enum(),
                                ))),
                            };
                            super::cast_lib::CastMemberRef {
                                cast_lib,
                                cast_member: member_datum.int_value().unwrap_or(0),
                            }
                        } else { INVALID_CAST_MEMBER_REF },
                    };
                    Ok::<_, ScriptError>(p.alloc_datum(Datum::CastMember(member_ref)))
                }).ok_or_else(crate::player::cancelled_scope_error).and_then(|r| r);
                let value = eval_turn_try!(result);
                self.values.push(eval_turn_try!(Self::validate_ref(session, self.player_id, &value)));
            }
            EvalFrame::ApplyDeleteChunk{chunk_type,has_end,source} => {
                let source_ref=eval_turn_try!(self.pop_value());let end=if has_end{eval_turn_try!(self.pop_value())}else{DatumRef::Void};let index=eval_turn_try!(self.pop_value());
                eval_turn_try!(Self::validate_ref(session, self.player_id, &source_ref));
                eval_turn_try!(Self::validate_ref(session, self.player_id, &index));
                if has_end { eval_turn_try!(Self::validate_ref(session, self.player_id, &end)); }
                let result=session.with_player(self.player_id,|context|{let p=context.player;let i=p.get_datum(&index).int_value()?;let e=if has_end{p.get_datum(&end).int_value()?}else{i};let text=p.get_datum(&source_ref).string_value(context.symbols)?;let chunk=StringChunkExpr{chunk_type:StringChunkType::from_symbol(&chunk_type,context.symbols)?,start:i,end:e,item_delimiter:p.movie.item_delimiter};let new_text=StringChunkUtils::string_by_deleting_chunk(&text,&chunk)?;write_chunk_source(p,context.symbols,&source,new_text);Ok::<_,ScriptError>(DatumRef::Void)}).ok_or_else(crate::player::cancelled_scope_error).and_then(|result|result);self.values.push(eval_turn_try!(result));
            }
            EvalFrame::ApplyIf{body}=>{let cond=eval_turn_try!(self.pop_value());let yes=eval_turn_try!(session.with_player(self.player_id,|context|context.player.get_datum(&cond).bool_value()).ok_or_else(crate::player::cancelled_scope_error).and_then(|result| result));if yes{self.frames.push(EvalFrame::Evaluate(body));}else{self.values.push(DatumRef::Void);}}
        }
        None
    }

    fn run_frames(&mut self, session:&mut crate::player::session::RuntimeSession)->EvalTurn {
        while let Some(frame)=self.frames.pop() {
            let pending=match frame {EvalFrame::Evaluate(expr)=>self.evaluate_frame(session,EvalFrame::Evaluate(expr)),other=>self.apply_frame(session,other)};
            if let Some(turn)=pending{return turn;}
        }
        EvalTurn::Complete(self.pop_value())
    }
}

pub fn eval_lingo_expr_static(
    expr: String,
    player: &mut DirPlayer,
    symbols: &mut SymbolTable,
) -> Result<DatumRef, ScriptError> {
    let _tokens = tokenize_lingo(&expr);
    match LingoParser::parse(Rule::eval_expr, expr.as_str()) {
        Ok(parse_result) => {
            let expr_pair = &parse_result.enumerate().next().unwrap();
            eval_lingo_pair_static(expr_pair.1.clone(), player, symbols)
        }
        Err(e) => {
            let error_msg = format!("eval_lingo_expr_static parse error: {}", ascii_safe(&e.to_string()));
            error!("{}", error_msg);
            web_sys::console::error_1(&error_msg.clone().into());
            Err(ScriptError::new(error_msg))
        }
    }
}

/// Like `eval_lingo_expr_static` but does not log errors on parse failure.
pub fn try_eval_lingo_expr_static(
    expr: String,
    player: &mut DirPlayer,
    symbols: &mut SymbolTable,
) -> Result<DatumRef, ScriptError> {
    let _tokens = tokenize_lingo(&expr);
    match LingoParser::parse(Rule::eval_expr, expr.as_str()) {
        Ok(parse_result) => {
            let expr_pair = &parse_result.enumerate().next().unwrap();
            eval_lingo_pair_static(expr_pair.1.clone(), player, symbols)
        }
        Err(e) => {
            Err(ScriptError::new(format!("eval_lingo_expr_static parse error: {}", ascii_safe(&e.to_string()))))
        }
    }
}

pub fn parse_lingo_expr_ast_runtime(rule: Rule, expr: String) -> Result<LingoExpr, ScriptError> {
    let pratt = create_lingo_pratt_parser();
    let _tokens = tokenize_lingo(&expr);
    match LingoParser::parse(rule, expr.as_str()) {
        Ok(parse_result) => {
            let expr_pair = &parse_result.enumerate().next().unwrap();
            let mut ast = parse_lingo_rule_runtime(expr_pair.1.clone(), &pratt)?;

            // In command context, convert bare identifiers to handler calls
            if rule == Rule::command_eval_expr {
                if let LingoExpr::Identifier(name) = ast {
                    ast = LingoExpr::HandlerCall(name, vec![]);
                }
            }

            Ok(ast)
        }
        Err(e) => Err(ScriptError::new(ascii_safe(&e.to_string()))),
    }
}

fn create_lingo_pratt_parser() -> PrattParser<Rule> {
    PrattParser::new()
        .op(Op::infix(Rule::or_op, Assoc::Left))              // Lowest: or
        .op(Op::infix(Rule::and_op, Assoc::Left))             // and
        .op(Op::prefix(Rule::not_op))                         // not (prefix)
        .op(Op::infix(Rule::eq_op, Assoc::Left)               // = comparison
            | Op::infix(Rule::ne_op, Assoc::Left)             // <>
            | Op::infix(Rule::lt_op, Assoc::Left)             // 
            | Op::infix(Rule::gt_op, Assoc::Left)             // >
            | Op::infix(Rule::le_op, Assoc::Left)             // <=
            | Op::infix(Rule::ge_op, Assoc::Left))            // >=
        .op(Op::infix(Rule::join, Assoc::Left)                // & concatenation
            | Op::infix(Rule::join_pad, Assoc::Left))         // && padded concat
        .op(Op::infix(Rule::add, Assoc::Left)                 // +, -
            | Op::infix(Rule::subtract, Assoc::Left))
        .op(Op::infix(Rule::multiply, Assoc::Left)            // *, /, mod
            | Op::infix(Rule::divide, Assoc::Left)
            | Op::infix(Rule::mod_op, Assoc::Left))
        .op(Op::prefix(Rule::neg_op))                         // unary minus
        .op(Op::infix(Rule::obj_prop, Assoc::Left)            // Highest: .
            | Op::postfix(Rule::list_index))                  // and [index]
}

// Helper functions for testing config parsing without requiring player instance
/// Parse a config value to check if it's valid Lingo syntax
/// Returns Ok if the value can be parsed, Err with parse error message if not
pub fn test_parse_lingo_value(value_str: &str) -> Result<(), String> {
    validate_lingo_expr_syntax(value_str)
}

/// Validate expression syntax without allocating a player, symbol table, or
/// runtime datum. File parsers use this boundary for values that are only
/// materialized once an owning runtime context exists.
pub fn validate_lingo_expr_syntax(value_str: &str) -> Result<(), String> {
    LingoParser::parse(Rule::eval_expr, value_str)
        .map(|_| ())
        .map_err(|e| format!("{}", e))
}
/// Parse a config key to check if it's valid
/// Returns Ok if the key can be parsed, Err with parse error message if not
pub fn test_parse_config_key(key_str: &str) -> Result<(), String> {
    LingoParser::parse(Rule::config_key, key_str)
        .map(|_| ())
        .map_err(|e| format!("{}", e))
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::player::ownership::OwnerToken;
    use async_std::channel;

    fn test_session() -> crate::player::session::RuntimeSession {
        let mut session = crate::player::session::RuntimeSession::new(
            crate::player::symbols::symbol_table::SymbolOwner {
                session: 900,
                generation: 1,
            },
        );
        let (tx, _rx) = channel::unbounded();
        assert!(session.add_player(1, tx));
        session
    }

    fn test_player() -> DirPlayer {
        let (tx, _rx) = channel::unbounded();
        DirPlayer::new_with_owner(tx, OwnerToken::transitional())
    }

    #[test]
    fn static_eval_uses_explicit_context_for_nested_values() {
        let mut player = test_player();
        let mut symbols = SymbolTable::new();
        let value = eval_lingo_expr_static(
            "[#MiXeD, point(1, 2)]".to_string(),
            &mut player,
            &mut symbols,
        )
        .expect("nested static expression should evaluate");
        let datum = checked_static_datum(&player, &value, &symbols)
            .expect("result should belong to the supplied session");
        let Datum::List(_, values, _) = datum else {
            panic!("expected nested list result");
        };
        let Datum::Symbol(symbol) = checked_static_datum(&player, &values[0], &symbols)
            .expect("nested symbol should be valid") else {
            panic!("expected symbol literal");
        };
        assert_eq!(symbols.display(&symbol).unwrap(), "MiXeD");
    }

    #[test]
    fn static_eval_rejects_symbol_from_another_session() {
        let mut player = test_player();
        let mut symbols = SymbolTable::new();
        let mut foreign_symbols = SymbolTable::new();
        let foreign = foreign_symbols.intern("foreign");
        let value = player.alloc_datum(Datum::Symbol(foreign));
        assert!(checked_static_datum(&player, &value, &symbols).is_err());
    }

    #[test]
    fn parser_uses_builtin_chunk_type_without_session_context() {
        let ast = parse_lingo_expr_ast_runtime(
            Rule::eval_expr,
            "item 1 of \"a,b\"".to_string(),
        )
        .expect("chunk expression should parse");
        let LingoExpr::ChunkExpr(chunk_type, ..) = ast else {
            panic!("expected chunk expression");
        };
        assert_eq!(chunk_type.into_builtin(), Some(BuiltInSymbol::Item));
    }

    #[test]
    fn static_eval_evaluates_nested_arithmetic_in_explicit_context() {
        let mut player = test_player();
        let mut symbols = SymbolTable::new();
        let value = eval_lingo_expr_static(
            "[1 + 2, point(3 * 4, 5)]".to_string(),
            &mut player,
            &mut symbols,
        )
        .expect("nested arithmetic should evaluate");
        let Datum::List(_, values, _) = checked_static_datum(&player, &value, &symbols)
            .expect("result should belong to the supplied session") else {
            panic!("expected nested list result");
        };
        assert!(matches!(
            checked_static_datum(&player, &values[0], &symbols),
            Ok(Datum::Int(3))
        ));
        assert!(matches!(
            checked_static_datum(&player, &values[1], &symbols),
            Ok(Datum::Point([x, y], _)) if x == 12.0 && y == 5.0
        ));
    }

    #[test]
    fn static_eval_rejects_foreign_symbol_in_global_value() {
        let mut player = test_player();
        let mut symbols = SymbolTable::new();
        let mut foreign_symbols = SymbolTable::new();
        let foreign = foreign_symbols.intern("foreign");
        let global_name = symbols.intern("foreign_global");
        let global_value = player.alloc_datum(Datum::Symbol(foreign));
        player.globals.insert(global_name, global_value);

        let result = eval_lingo_expr_static(
            "foreign_global".to_string(),
            &mut player,
            &mut symbols,
        );
        assert!(result.is_err(), "foreign global values must fail through eval");
    }

    #[test]
    fn static_eval_unknown_identifier_is_void() {
        let mut player = test_player();
        let mut symbols = SymbolTable::new();
        let value = eval_lingo_expr_static(
            "not_declared".to_string(),
            &mut player,
            &mut symbols,
        )
        .expect("ordinary unknown identifiers should use Director's VOID fallback");
        assert!(matches!(
            checked_static_datum(&player, &value, &symbols),
            Ok(Datum::Void)
        ));
    }

    #[test]
    fn owned_eval_preserves_noncommutative_and_list_order() {
        let mut session = test_session();
        let subtract = session
            .start_eval(
                1,
                LingoExpr::Subtract(
                    Box::new(LingoExpr::IntLiteral(10)),
                    Box::new(LingoExpr::IntLiteral(3)),
                ),
            )
            .unwrap();
        let result = match session.turn_eval(subtract) {
            EvalTurn::Complete(Ok(value)) => session
                .with_player(1, |context| context.player.get_datum(&value).clone())
                .unwrap(),
            other => panic!("unexpected subtraction turn: {:?}", std::mem::discriminant(&other)),
        };
        assert!(matches!(result, Datum::Int(7)));

        let list = session
            .start_eval(
                1,
                LingoExpr::ListLiteral(vec![
                    LingoExpr::IntLiteral(1),
                    LingoExpr::IntLiteral(2),
                    LingoExpr::IntLiteral(3),
                ]),
            )
            .unwrap();
        let result = match session.turn_eval(list) {
            EvalTurn::Complete(Ok(value)) => session
                .with_player(1, |context| context.player.get_datum(&value).clone())
                .unwrap(),
            other => panic!("unexpected list turn: {:?}", std::mem::discriminant(&other)),
        };
        let Datum::List(_, values, _) = result else { panic!("expected list") };
        let ints: Vec<_> = values
            .iter()
            .map(|value| {
                session
                    .with_player(1, |context| context.player.get_datum(value).int_value())
                    .unwrap()
                    .unwrap()
            })
            .collect();
        assert_eq!(ints, vec![1, 2, 3]);
    }

    #[test]
    fn static_eval_not_true_is_false() {
        let mut session = test_session();
        let result = session
            .with_player(1, |context| {
                let value = eval_lingo_expr_static("not true".to_owned(), context.player, context.symbols)?;
                let nested = eval_lingo_expr_static("not not true".to_owned(), context.player, context.symbols)?;
                let parenthesized = eval_lingo_expr_static("not (not true)".to_owned(), context.player, context.symbols)?;
                let rhs = eval_lingo_expr_static("1 + not false".to_owned(), context.player, context.symbols)?;
                Ok::<_, ScriptError>((
                    context.player.get_datum(&value).clone(),
                    context.player.get_datum(&nested).clone(),
                    context.player.get_datum(&parenthesized).clone(),
                    context.player.get_datum(&rhs).clone(),
                ))
            })
            .unwrap()
            .unwrap();
        assert!(matches!(result.0, Datum::Int(0)));
        assert!(matches!(result.1, Datum::Int(1)));
        assert!(matches!(result.2, Datum::Int(1)));
        assert!(matches!(result.3, Datum::Int(2)));
    }

    #[test]
    fn owned_eval_pending_keeps_args_and_resumes_parent_frames_once() {
        let mut session = test_session();
        let id = session
            .start_eval(
                1,
                LingoExpr::Subtract(
                    Box::new(LingoExpr::ObjHandlerCall(
                        Box::new(LingoExpr::IntLiteral(1)),
                        "deferred".to_owned(),
                        vec![LingoExpr::IntLiteral(4), LingoExpr::IntLiteral(5)],
                    )),
                    Box::new(LingoExpr::IntLiteral(2)),
                ),
            )
            .unwrap();
        let pending = session.turn_eval(id.clone());
        let EvalTurn::Pending {
            request:
                EvalPending::Object {
                    capability,
                        request:
                            crate::player::driver::InternalVmRequest::Object {
                                receiver,
                                args,
                                ..
                            },
                        ..
                },
        } = pending
        else {
            panic!("expected an owned object request")
        };
        assert_eq!(capability.serial, 1);
        let values: Vec<_> = session
            .with_player(1, |context| {
                [receiver, args[0].clone(), args[1].clone()]
                    .into_iter()
                    .map(|value| context.player.get_datum(&value).int_value().unwrap())
                    .collect()
            })
            .unwrap();
        assert_eq!(values, vec![1, 4, 5]);

        let owner = session.with_player(1, |context| context.player.owner.clone()).unwrap();
        let completion = session
            .with_player(1, |context| context.player.alloc_datum(Datum::Int(10)))
            .unwrap();
        let result = session.resume_eval(
            id.clone(),
            &capability,
            &owner,
            Ok(completion),
        );
        let EvalTurn::Complete(Ok(value)) = result else { panic!("expected resumed completion") };
        let result = session.with_player(1, |context| context.player.get_datum(&value).clone()).unwrap();
        assert!(matches!(result, Datum::Int(8)));
        assert!(matches!(session.turn_eval(id), EvalTurn::Complete(Err(_))));
    }

    #[test]
    fn owned_eval_rejects_foreign_completion_without_consuming_pending_request() {
        let mut session = test_session();
        let id = session
            .start_eval(
                1,
                LingoExpr::ObjHandlerCall(
                    Box::new(LingoExpr::IntLiteral(1)),
                    "deferred".to_owned(),
                    vec![],
                ),
            )
            .unwrap();
        let pending = session.turn_eval(id.clone());
        let action = match pending {
            EvalTurn::Pending { request: EvalPending::Object { capability, .. } } => capability,
            _ => panic!("expected an owned pending action"),
        };
        assert!(matches!(session.turn_eval(id.clone()), EvalTurn::Complete(Err(_))));
        let foreign = OwnerToken::transitional();
        let foreign_value = session
            .with_player(1, |context| context.player.alloc_datum(Datum::Int(99)))
            .unwrap();
        assert!(matches!(
            session.resume_eval(id.clone(), &action, &foreign, Ok(foreign_value)),
            EvalTurn::Complete(Err(_))
        ));
        let owner = session.with_player(1, |context| context.player.owner.clone()).unwrap();
        let value = session
            .with_player(1, |context| context.player.alloc_datum(Datum::Int(7)))
            .unwrap();
        assert!(matches!(session.resume_eval(id.clone(), &action, &owner, Ok(value)), EvalTurn::Complete(Ok(_))));
        assert!(matches!(session.resume_eval(id, &action, &owner, Ok(DatumRef::Void)), EvalTurn::Complete(Err(_))));
    }

    #[test]
    fn owned_eval_rejects_previous_action_after_next_wait_is_issued() {
        let mut session = test_session();
        let call = |name: &str| {
            LingoExpr::ObjHandlerCall(
                Box::new(LingoExpr::IntLiteral(1)), name.to_owned(), vec![],
            )
        };
        let id = session
            .start_eval(1, LingoExpr::Add(Box::new(call("first")), Box::new(call("second"))))
            .unwrap();
        let first = match session.turn_eval(id.clone()) {
            EvalTurn::Pending { request: EvalPending::Object { capability, .. } } => capability,
            _ => panic!("expected first pending action"),
        };
        let owner = session.with_player(1, |context| context.player.owner.clone()).unwrap();
        let first_value = session.with_player(1, |context| context.player.alloc_datum(Datum::Int(3))).unwrap();
        let second = match session.resume_eval(id.clone(), &first, &owner, Ok(first_value)) {
            EvalTurn::Pending { request: EvalPending::Object { capability, .. } } => capability,
            _ => panic!("expected second pending action"),
        };
        assert_ne!(first, second);
        assert!(matches!(
            session.resume_eval(id.clone(), &first, &owner, Ok(DatumRef::Void)),
            EvalTurn::Complete(Err(_))
        ));
        let second_value = session.with_player(1, |context| context.player.alloc_datum(Datum::Int(4))).unwrap();
        assert!(matches!(session.resume_eval(id, &second, &owner, Ok(second_value)), EvalTurn::Complete(Ok(_))));
    }
    #[test]
    fn owned_eval_rejects_foreign_element_exposed_by_local_list() {
        let mut session = test_session();
        let mut foreign_symbols = SymbolTable::new();
        let foreign_symbol = foreign_symbols.intern("foreign_element");
        let list_name = session.with_player(1, |context| {
            let foreign = context.player.alloc_datum(Datum::Symbol(foreign_symbol));
            let list = context.player.alloc_datum(Datum::List(
                DatumType::List,
                vec![foreign].into_iter().collect(),
                false,
            ));
            let name = context.symbols.intern("local_list");
            context.player.globals.insert(name.clone(), list);
            name
        }).unwrap();
        let name = session.with_player(1, |context| context.symbols.display(&list_name).unwrap().to_owned()).unwrap();
        let id = session.start_eval(1, LingoExpr::ListAccess(
            Box::new(LingoExpr::Identifier(name)),
            Box::new(LingoExpr::IntLiteral(1)),
        )).unwrap();
        assert!(matches!(session.turn_eval(id), EvalTurn::Complete(Err(error)) if error.code == crate::player::ScriptErrorCode::InvalidReference));
    }

    #[test]
    fn rejected_complete_eval_keeps_pending_ticket() {
        let mut session = test_session();
        let id = session.start_eval(1, LingoExpr::ObjHandlerCall(
            Box::new(LingoExpr::IntLiteral(1)), "deferred".to_owned(), vec![],
        )).unwrap();
        let action = match session.turn_eval(id.clone()) {
            EvalTurn::Pending { request: EvalPending::Object { capability, .. } } => capability,
            _ => panic!("expected pending object request"),
        };
        let foreign_owner = OwnerToken::transitional();
        assert!(!session.complete_eval(id.clone(), &action, &foreign_owner, Ok(DatumRef::Void)));
        let owner = session.with_player(1, |context| context.player.owner.clone()).unwrap();
        let value = session.with_player(1, |context| context.player.alloc_datum(Datum::Int(9))).unwrap();
        assert!(session.complete_eval(id.clone(), &action, &owner, Ok(value)));
        assert!(matches!(session.turn_eval(id), EvalTurn::Complete(Ok(_))));
    }

    #[test]
    fn owned_eval_root_completion_returns_value() {
        let mut session = test_session();
        let id = session.start_eval(1, LingoExpr::ObjHandlerCall(
            Box::new(LingoExpr::IntLiteral(1)), "deferred".to_owned(), vec![],
        )).unwrap();
        let action = match session.turn_eval(id.clone()) {
            EvalTurn::Pending { request: EvalPending::Object { capability, .. } } => capability,
            _ => panic!("expected pending root handler"),
        };
        let owner = session.with_player(1, |context| context.player.owner.clone()).unwrap();
        let value = session.with_player(1, |context| context.player.alloc_datum(Datum::Int(9))).unwrap();
        let result = session.resume_eval(id, &action, &owner, Ok(value));
        let EvalTurn::Complete(Ok(result)) = result else { panic!("expected root result") };
        assert!(matches!(session.with_player(1, |context| context.player.get_datum(&result).clone()).unwrap(), Datum::Int(9)));
    }

    #[test]
    fn owned_command_last_pending_line_returns_its_completion() {
        let mut session = test_session();
        let id = session.start_command_eval(1, vec![
            "sendAllSprites(#first)".to_owned(),
            "sendAllSprites(#last)".to_owned(),
        ]).unwrap();
        let first = match session.turn_eval(id.clone()) {
            EvalTurn::Pending { request: EvalPending::Global { capability, .. } } => capability,
            _ => panic!("expected first command to suspend"),
        };
        let owner = session.with_player(1, |context| context.player.owner.clone()).unwrap();
        let first_value = session.with_player(1, |context| context.player.alloc_datum(Datum::Int(4))).unwrap();
        let second = match session.resume_eval(id.clone(), &first, &owner, Ok(first_value)) {
            EvalTurn::Pending { request: EvalPending::Global { capability, .. } } => capability,
            _ => panic!("expected second command to suspend"),
        };
        let last_value = session.with_player(1, |context| context.player.alloc_datum(Datum::Int(11))).unwrap();
        let result = session.resume_eval(id, &second, &owner, Ok(last_value));
        let EvalTurn::Complete(Ok(result)) = result else { panic!("expected last command result") };
        assert!(matches!(session.with_player(1, |context| context.player.get_datum(&result).clone()).unwrap(), Datum::Int(11)));
    }

    fn datum_result(session: &mut crate::player::session::RuntimeSession, id: EvalId) -> Datum {
        let result = match session.turn_eval(id) {
            EvalTurn::Complete(Ok(value)) => value,
            EvalTurn::Complete(Err(error)) => panic!("evaluation failed: {error:?}"),
            EvalTurn::Pending { .. } => panic!("test expression unexpectedly suspended"),
        };
        session.with_player(1, |context| context.player.get_datum(&result).clone()).unwrap()
    }

    fn global_datum(session: &mut crate::player::session::RuntimeSession, name: &str) -> Option<Datum> {
        session.with_player(1, |context| {
            let symbol = context.symbols.intern(name);
            context.player.globals.get(&symbol).map(|value| context.player.get_datum(value).clone())
        }).unwrap()
    }

    fn item_chunk(index: i32, end: Option<i32>, source: &str) -> LingoExpr {
        LingoExpr::ChunkExpr(
            Symbol::builtin(BuiltInSymbol::Item),
            Box::new(LingoExpr::IntLiteral(index)),
            end.map(|value| Box::new(LingoExpr::IntLiteral(value))),
            Box::new(LingoExpr::Identifier(source.to_owned())),
        )
    }

    #[test]
    fn owned_eval_chunk_read_preserves_ranged_output() {
        let mut session = test_session();
        session.with_player(1, |context| {
            let symbol = context.symbols.intern("read_source");
            let value = context.player.alloc_datum(Datum::String("a,b,c,d".to_owned()));
            context.player.globals.insert(symbol, value);
        }).unwrap();
        let id = session.start_eval(1, item_chunk(2, Some(3), "read_source")).unwrap();
        let result = datum_result(&mut session, id);
        assert!(matches!(result, Datum::String(value) if value == "b,c"));
    }

    #[test]
    fn owned_eval_chunk_delete_and_put_preserve_source_and_concat_formatting() {
        let mut session = test_session();
        let set_source = |session: &mut crate::player::session::RuntimeSession, value: &str| {
            session.with_player(1, |context| {
                let symbol = context.symbols.intern("chunk_source");
                let value = context.player.alloc_datum(Datum::String(value.to_owned()));
                context.player.globals.insert(symbol, value);
            }).unwrap();
        };

        set_source(&mut session, "a,b,c,d");
        let deleted = session.start_eval(1, LingoExpr::DeleteChunk(Box::new(item_chunk(2, Some(3), "chunk_source")))).unwrap();
        assert!(matches!(datum_result(&mut session, deleted), Datum::Void));
        assert!(matches!(global_datum(&mut session, "chunk_source"), Some(Datum::String(value)) if value == "a,d"));

        set_source(&mut session, "a,b,c");
        let numeric = session.start_eval(1, LingoExpr::PutBefore(
            Box::new(LingoExpr::IntLiteral(2)),
            Box::new(item_chunk(2, None, "chunk_source")),
        )).unwrap();
        assert!(matches!(datum_result(&mut session, numeric), Datum::Void));
        assert!(matches!(global_datum(&mut session, "chunk_source"), Some(Datum::String(value)) if value == "a,2b,c"));

        set_source(&mut session, "a,b,c");
        let list = session.start_eval(1, LingoExpr::PutBefore(
            Box::new(LingoExpr::ListLiteral(vec![LingoExpr::IntLiteral(1), LingoExpr::IntLiteral(2)])),
            Box::new(item_chunk(2, None, "chunk_source")),
        )).unwrap();
        assert!(matches!(datum_result(&mut session, list), Datum::Void));
        assert!(matches!(global_datum(&mut session, "chunk_source"), Some(Datum::String(value)) if value == "a,[1, 2]b,c"));
    }

    #[test]
    fn owned_eval_invalid_chunk_start_skips_later_end_side_effect() {
        let mut session = test_session();
        let chunk = LingoExpr::ChunkExpr(
            Symbol::builtin(BuiltInSymbol::Item),
            Box::new(LingoExpr::ListLiteral(vec![LingoExpr::IntLiteral(1)])),
            Some(Box::new(LingoExpr::Assignment(
                Box::new(LingoExpr::Identifier("end_side_effect".to_owned())),
                Box::new(LingoExpr::IntLiteral(7)),
            ))),
            Box::new(LingoExpr::StringLiteral("a,b".to_owned())),
        );
        let id = session.start_eval(1, chunk).unwrap();
        assert!(matches!(session.turn_eval(id), EvalTurn::Complete(Err(_))));
        assert!(global_datum(&mut session, "end_side_effect").is_none());
    }

    #[test]
    fn owned_eval_chunk_fallback_re_evaluates_original_target() {
        let mut session = test_session();
        let initialize = session.start_eval(1, LingoExpr::Assignment(
            Box::new(LingoExpr::Identifier("fallback_count".to_owned())),
            Box::new(LingoExpr::IntLiteral(0)),
        )).unwrap();
        let _ = datum_result(&mut session, initialize);
        let target = LingoExpr::ListAccess(
            Box::new(LingoExpr::ObjProp(
                Box::new(LingoExpr::Assignment(
                    Box::new(LingoExpr::Identifier("fallback_count".to_owned())),
                    Box::new(LingoExpr::Add(
                        Box::new(LingoExpr::Identifier("fallback_count".to_owned())),
                        Box::new(LingoExpr::IntLiteral(1)),
                    )),
                )),
                "line".to_owned(),
            )),
            Box::new(LingoExpr::IntLiteral(1)),
        );
        let id = session.start_eval(1, target).unwrap();
        assert!(matches!(session.turn_eval(id), EvalTurn::Complete(Err(_))));
        assert!(matches!(global_datum(&mut session, "fallback_count"), Some(Datum::Int(2))));
    }

    #[test]
    fn owned_command_lazily_reports_later_parse_error_after_prior_assignment() {
        let mut session = test_session();
        let id = session.start_command_eval(1, vec![
            "put 7 into prior_assignment".to_owned(),
            "put ???".to_owned(),
        ]).unwrap();
        assert!(matches!(session.turn_eval(id), EvalTurn::Complete(Err(_))));
        assert!(matches!(global_datum(&mut session, "prior_assignment"), Some(Datum::Int(7))));
    }

}
