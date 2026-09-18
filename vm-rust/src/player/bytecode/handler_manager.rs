use std::rc::Rc;

use crate::{
    director::{
        chunks::handler::HandlerDef,
        lingo::{constants::get_opcode_name, opcode::OpCode},
    },
    player::{
        bytecode::{
            arithmetics::ArithmeticsBytecodeHandler, flow_control::FlowControlBytecodeHandler,
            stack::StackBytecodeHandler,
        },
        scope::ScopeRef,
        script::Script,
        session::ExecutionContext,
        symbols::symbol::Symbol,
        HandlerExecutionResult, ScopeToken, ScriptError, PLAYER_OPT,
    },
};

use super::{
    compare::CompareBytecodeHandler, expression_tracker::StackExpressionTracker,
    get_set::GetSetBytecodeHandler, sprite_compare::SpriteCompareBytecodeHandler,
    string::StringBytecodeHandler,
};

thread_local! {
    pub static EXPRESSION_TRACKER: std::cell::RefCell<StackExpressionTracker> =
        std::cell::RefCell::new(StackExpressionTracker::new());
}
/// Lightweight execution history entry - stores only minimal data
/// Strings are generated lazily only when dumping on error
#[derive(Clone, Copy)]
struct ExecutionHistoryEntry {
    opcode: u16,
    bytecode_pos: u32,
    operand: i32,
    handler_name_id: u32,
    script_cast_lib: u32,
    script_cast_member: i32,
}

/// Ring buffer for execution history with minimal memory footprint
const EXECUTION_HISTORY_SIZE: usize = 100;

thread_local! {
    // UnsafeCell, not RefCell: this is written on EVERY bytecode op (~1M/frame),
    // so the RefCell borrow-flag check was pure per-op overhead. wasm is single-
    // threaded and `push`/reads never re-enter, so direct access is sound.
    static EXECUTION_HISTORY: std::cell::UnsafeCell<ExecutionHistory> =
        std::cell::UnsafeCell::new(ExecutionHistory::new());
}

struct ExecutionHistory {
    entries: [ExecutionHistoryEntry; EXECUTION_HISTORY_SIZE],
    write_index: usize,
    count: usize,
}

impl ExecutionHistory {
    const fn new() -> Self {
        Self {
            entries: [ExecutionHistoryEntry {
                opcode: 0,
                bytecode_pos: 0,
                operand: 0,
                handler_name_id: 0,
                script_cast_lib: 0,
                script_cast_member: 0,
            }; EXECUTION_HISTORY_SIZE],
            write_index: 0,
            count: 0,
        }
    }

    #[inline(always)]
    fn push(&mut self, entry: ExecutionHistoryEntry) {
        self.entries[self.write_index] = entry;
        self.write_index = (self.write_index + 1) % EXECUTION_HISTORY_SIZE;
        if self.count < EXECUTION_HISTORY_SIZE {
            self.count += 1;
        }
    }

    fn iter_recent(&self) -> impl Iterator<Item = &ExecutionHistoryEntry> {
        let start = if self.count < EXECUTION_HISTORY_SIZE {
            0
        } else {
            self.write_index
        };

        (0..self.count).map(move |i| {
            let idx = (start + i) % EXECUTION_HISTORY_SIZE;
            &self.entries[idx]
        })
    }
}

/// Records a bytecode execution to the history (lightweight - no string allocation)
#[inline(always)]
fn record_execution(
    opcode_u16: u16,
    bytecode_pos: u32,
    operand: i32,
    handler_name_id: u32,
    script_cast_lib: u32,
    script_cast_member: i32,
) {
    EXECUTION_HISTORY.with(|history| {
        // SAFETY: single-threaded wasm; push does not re-enter or alias.
        unsafe {
            (*history.get()).push(ExecutionHistoryEntry {
                opcode: opcode_u16,
                bytecode_pos,
                operand,
                handler_name_id,
                script_cast_lib,
                script_cast_member,
            });
        }
    });
}

/// Dumps the execution history when an error occurs
/// This is the only place where strings are generated - on demand
pub fn dump_execution_history_on_error(error_message: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        use web_sys::console;

        console::group_collapsed_1(
            &format!(
                "📜 Bytecode execution history (last {} ops before error)",
                EXECUTION_HISTORY_SIZE
            )
            .into(),
        );
        console::error_1(&format!("Error: {}", error_message).into());

        EXECUTION_HISTORY.with(|history| {
            // SAFETY: single-threaded; no concurrent writer during error dump.
            let history = unsafe { &*history.get() };
            let player = unsafe { PLAYER_OPT.as_ref() };

            for (i, entry) in history.iter_recent().enumerate() {
                // Convert opcode back to name using num_traits
                let opcode: OpCode =
                    num::FromPrimitive::from_u16(entry.opcode).unwrap_or(OpCode::Invalid);
                let op_name = get_opcode_name(opcode);

                // Try to get handler name from lctx if player is available
                let handler_name = if let Some(player) = player {
                    player
                        .movie
                        .cast_manager
                        .get_cast(entry.script_cast_lib)
                        .ok()
                        .and_then(|cast| cast.lctx.as_ref())
                        .and_then(|lctx| lctx.names.get(entry.handler_name_id as usize))
                        .map(|s| s.as_str())
                        .unwrap_or("?")
                } else {
                    "?"
                };

                let msg = format!(
                    "{:3}. [{:4}] {:20} {:6} ({}@{}:{})",
                    i + 1,
                    entry.bytecode_pos,
                    op_name,
                    entry.operand,
                    handler_name,
                    entry.script_cast_lib,
                    entry.script_cast_member
                );
                console::log_1(&msg.into());
            }
        });

        console::group_end();
    }
}

// trace_output is imported from crate::player

#[derive(Clone)]
pub(crate) struct HandlerCode {
    pub(crate) script: Rc<Script>,
    pub(crate) handler: Rc<HandlerDef>,
    pub(crate) names: Rc<[Symbol]>,
}

#[derive(Clone)]
pub struct BytecodeHandlerContext {
    pub(crate) scope: ScopeToken,
    pub(crate) code: HandlerCode,
    /// Variable-index multiplier for this script (constant for the whole
    /// handler). Cached here so per-opcode variable handlers (getlocal/
    /// setlocal/getparam/getglobal/pushcons/...) don't re-derive it via a
    /// cast lookup on every op — that was pure per-op overhead in tight loops.
    pub(crate) multiplier: u32,
}

impl BytecodeHandlerContext {
    #[inline(always)]
    pub(crate) fn scope_ref(&self) -> ScopeRef {
        self.scope.slot()
    }
}

impl BytecodeHandlerContext {
    /// Get a name from the name table by ID without borrowing player.
    #[inline(always)]
    pub fn get_name(&self, name_id: u16) -> Symbol {
        self.code.names[name_id as usize].clone()
    }
}
pub struct StaticBytecodeHandlerManager {}
impl StaticBytecodeHandlerManager {
    #[inline(always)]
    pub fn call_sync_handler(
        opcode: OpCode,
        runtime: &mut ExecutionContext<'_>,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        match opcode {
            OpCode::Add => ArithmeticsBytecodeHandler::add(runtime, ctx),
            OpCode::PushInt8 => StackBytecodeHandler::push_int(runtime, ctx),
            OpCode::PushInt16 => StackBytecodeHandler::push_int(runtime, ctx),
            OpCode::PushInt32 => StackBytecodeHandler::push_int(runtime, ctx),
            OpCode::PushArgList => StackBytecodeHandler::push_arglist(runtime, ctx),
            OpCode::PushArgListNoRet => StackBytecodeHandler::push_arglist_no_ret(runtime, ctx),
            OpCode::PushSymb => StackBytecodeHandler::push_symb(runtime, ctx),
            OpCode::Swap => StackBytecodeHandler::swap(runtime, ctx),
            OpCode::PushVarRef => StackBytecodeHandler::push_var_ref(runtime, ctx),
            OpCode::GetProp => GetSetBytecodeHandler::get_prop(runtime, ctx),
            OpCode::GetObjProp => GetSetBytecodeHandler::get_obj_prop(runtime, ctx),
            OpCode::GetMovieProp => GetSetBytecodeHandler::get_movie_prop(runtime, ctx),
            OpCode::Set => GetSetBytecodeHandler::set(runtime, ctx),
            OpCode::Ret => FlowControlBytecodeHandler::ret(runtime, ctx),
            OpCode::LocalCall => FlowControlBytecodeHandler::local_call(runtime, ctx),
            OpCode::JmpIfZ => FlowControlBytecodeHandler::jmp_if_zero(runtime, ctx),
            OpCode::Jmp => FlowControlBytecodeHandler::jmp(runtime, ctx),
            // GetGlobal2 (0x48) / SetGlobal2 (0x4e) are alternate encodings of
            // GetGlobal/SetGlobal emitted by older (D4) compilers; same
            // semantics. hackey initMain uses setGlobal2.
            OpCode::GetGlobal | OpCode::GetGlobal2 => {
                GetSetBytecodeHandler::get_global(runtime, ctx)
            }
            OpCode::SetGlobal | OpCode::SetGlobal2 => {
                GetSetBytecodeHandler::set_global(runtime, ctx)
            }
            OpCode::PushCons => StackBytecodeHandler::push_cons(runtime, ctx),
            OpCode::PushZero => StackBytecodeHandler::push_zero(runtime, ctx),
            OpCode::GetField => GetSetBytecodeHandler::get_field(runtime, ctx),
            OpCode::GetLocal => GetSetBytecodeHandler::get_local(runtime, ctx),
            OpCode::SetLocal => GetSetBytecodeHandler::set_local(runtime, ctx),
            OpCode::GetParam => GetSetBytecodeHandler::get_param(runtime, ctx),
            OpCode::SetMovieProp => GetSetBytecodeHandler::set_movie_prop(runtime, ctx),
            OpCode::PushPropList => StackBytecodeHandler::push_prop_list(runtime, ctx),
            OpCode::Gt => CompareBytecodeHandler::gt(runtime, ctx),
            OpCode::Lt => CompareBytecodeHandler::lt(runtime, ctx),
            OpCode::GtEq => CompareBytecodeHandler::gt_eq(runtime, ctx),
            OpCode::LtEq => CompareBytecodeHandler::lt_eq(runtime, ctx),
            OpCode::Sub => ArithmeticsBytecodeHandler::sub(runtime, ctx),
            OpCode::EndRepeat => FlowControlBytecodeHandler::end_repeat(runtime, ctx),
            OpCode::SetProp => GetSetBytecodeHandler::set_prop(runtime, ctx),
            OpCode::PushList => StackBytecodeHandler::push_list(runtime, ctx),
            OpCode::Not => CompareBytecodeHandler::not(runtime, ctx),
            OpCode::NtEq => CompareBytecodeHandler::nt_eq(runtime, ctx),
            OpCode::TheBuiltin => GetSetBytecodeHandler::the_built_in(runtime, ctx),
            OpCode::Peek => StackBytecodeHandler::peek(runtime, ctx),
            OpCode::Pop => StackBytecodeHandler::pop(runtime, ctx),
            OpCode::And => CompareBytecodeHandler::and(runtime, ctx),
            OpCode::Eq => CompareBytecodeHandler::eq(runtime, ctx),
            OpCode::SetParam => GetSetBytecodeHandler::set_param(runtime, ctx),
            OpCode::GetChainedProp => GetSetBytecodeHandler::get_chained_prop(runtime, ctx),
            OpCode::ContainsStr => StringBytecodeHandler::contains_str(runtime, ctx),
            OpCode::Contains0Str => StringBytecodeHandler::contains_0str(runtime, ctx),
            OpCode::JoinPadStr => StringBytecodeHandler::join_pad_str(runtime, ctx),
            OpCode::JoinStr => StringBytecodeHandler::join_str(runtime, ctx),
            OpCode::Get => GetSetBytecodeHandler::get(runtime, ctx),
            OpCode::Mod => ArithmeticsBytecodeHandler::mod_handler(runtime, ctx),
            OpCode::GetChunk => StringBytecodeHandler::get_chunk(runtime, ctx),
            OpCode::Put => StringBytecodeHandler::put(runtime, ctx),
            OpCode::Or => CompareBytecodeHandler::or(runtime, ctx),
            OpCode::Inv => ArithmeticsBytecodeHandler::inv(runtime, ctx),
            OpCode::Div => ArithmeticsBytecodeHandler::div(runtime, ctx),
            OpCode::PushFloat32 => StackBytecodeHandler::push_f32(runtime, ctx),
            OpCode::Mul => ArithmeticsBytecodeHandler::mul(runtime, ctx),
            OpCode::PushChunkVarRef => StackBytecodeHandler::push_chunk_var_ref(runtime, ctx),
            OpCode::PushVarRef => StackBytecodeHandler::push_var_ref(runtime, ctx),
            OpCode::DeleteChunk => StringBytecodeHandler::delete_chunk(runtime, ctx),
            OpCode::GetTopLevelProp => GetSetBytecodeHandler::get_top_level_prop(runtime, ctx),
            OpCode::PutChunk => StringBytecodeHandler::put_chunk(runtime, ctx),
            OpCode::OntoSpr => SpriteCompareBytecodeHandler::onto_sprite(runtime, ctx),
            OpCode::IntoSpr => SpriteCompareBytecodeHandler::into_sprite(runtime, ctx),
            OpCode::CallJavaScript => FlowControlBytecodeHandler::call_javascript(runtime, ctx),
            OpCode::StartTell => FlowControlBytecodeHandler::start_tell(runtime, ctx),
            OpCode::EndTell => FlowControlBytecodeHandler::end_tell(runtime, ctx),
            _ => {
                let prim = num::ToPrimitive::to_u16(&opcode).unwrap();
                let name = get_opcode_name(opcode);
                let fmt = format!("No handler for opcode {name} ({prim:#04x})");
                Err(ScriptError::new(fmt))
            }
        }
    }

    #[inline(always)]
    pub fn has_async_handler(opcode: &OpCode) -> bool {
        match opcode {
            OpCode::NewObj => true,
            OpCode::ExtCall => true,
            OpCode::ObjCall => true,
            OpCode::ObjCallV4 => true,
            OpCode::GetObjProp => true,
            OpCode::SetObjProp => true,
            OpCode::TellCall => true,
            _ => false,
        }
    }
}

/// Synchronous fast path for bytecode dispatch.
///
/// The vast majority of opcodes (pushes, gets/sets, arithmetic, jumps) are
/// synchronous, yet routing every one of them through the `async`
/// `player_execute_bytecode` + `.await` makes each op build and poll a future
/// state machine. That fixed per-op cost dominates the interpreter (the
/// throughput benchmark shows ~78 ns/op with the work itself being a small
/// fraction).
///
/// This reads the current opcode and, when it is NOT one of the handful of
/// genuinely-async opcodes (calls / NewObj / SetObjProp), executes it directly
/// and returns the result — no future, no await. It returns `None` when the
/// opcode needs the async path, so the caller falls back to
/// `player_execute_bytecode(&ctx).await`.
#[inline]
pub fn try_execute_bytecode_sync(
    runtime: &mut ExecutionContext<'_>,
    ctx: &BytecodeHandlerContext,
) -> Option<Result<HandlerExecutionResult, ScriptError>> {
    let opcode = {
        // The ACTIVE player, not PLAYER_OPT. PLAYER_OPT is always the host; a
        // nested #movie runs with ACTIVE_PLAYER_ID != 0, so reading the scope
        // from the host gave a DIFFERENT scope than the one being executed and
        // dispatched whatever opcode sat at the host scope's bytecode_index.
        // Neopets g349 (a #movie inside dgs_loader) died in its own prepareMovie
        // with "jmp_if_zero: stack underflow … bytecode_index=0" — index 0 being
        // the host scope, while the sub was mid-handler with an empty stack.
        let player = &*runtime.player;
        let scope = player.scopes.get(ctx.scope_ref()).unwrap();
        let handler = ctx.code.handler.as_ref();
        if scope.bytecode_index >= handler.bytecode_array.len() {
            return Some(Ok(HandlerExecutionResult::Stop));
        }
        handler.bytecode_array[scope.bytecode_index].opcode
    };
    try_execute_opcode_sync(opcode, runtime, ctx)
}

/// `try_execute_bytecode_sync` for a caller that has ALREADY decoded the
/// opcode. The driver loop reads the scope's `bytecode_index` every op anyway,
/// so having it re-read the scope here duplicated a lookup per opcode.
#[inline(always)]
pub fn try_execute_opcode_sync(
    opcode: OpCode,
    runtime: &mut ExecutionContext<'_>,
    ctx: &BytecodeHandlerContext,
) -> Option<Result<HandlerExecutionResult, ScriptError>> {
    if StaticBytecodeHandlerManager::has_async_handler(&opcode) {
        None
    } else {
        // Per-opcode profiling frame, but only build it when actually recording.
        // `get_opcode_name` is a HashMap lookup, so computing the frame name
        // eagerly would tax every op even in production. Gate on `is_recording`
        // so the non-recording fast path stays a single atomic load.
        let _op_scope = if crate::player::profiling::is_recording() {
            Some(crate::player::profiling::ProfileScope::new(
                crate::director::lingo::constants::get_opcode_name(opcode),
            ))
        } else {
            None
        };
        Some(StaticBytecodeHandlerManager::call_sync_handler(
            opcode, runtime, ctx,
        ))
    }
}
