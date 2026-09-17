use fxhash::FxHashMap;

use crate::director::lingo::datum::Datum;

use super::{
    allocator::{DatumAllocator, DatumAllocatorTrait},
    bitmap::manager::BitmapManager,
    cast_lib::{CastMemberRef, INVALID_CAST_MEMBER_REF},
    symbols::symbol::Symbol,
    script_ref::ScriptInstanceRef,
    DatumRef,
};

pub type ScopeRef = usize;

/// A value on the Lingo operand stack.
///
/// Primitive values (int/float/symbol/void) are stored INLINE, so pushing them
/// constructs neither a 64-byte `Datum` nor a `DatumRef` and never touches the
/// arena. A value is materialized into a real `DatumRef` only when something
/// actually needs one (popped/peeked by a consumer that works on `DatumRef`),
/// using the pooled fast paths (`alloc_int`/`alloc_symbol`) which return cached
/// immortal refs. Consumers that understand inline values (arithmetic, compare)
/// can read the primitive directly and skip materialization entirely.
#[derive(Clone)]
pub enum StackDatum {
    Int(i32),
    Float(f64),
    Symbol(Symbol),
    Void,
    Ref(DatumRef),
    /// A call's argument marker: its `count` arguments sit directly beneath it
    /// on the stack, and `no_ret` records whether the call discards its result.
    ///
    /// `pusharglist` used to pop the arguments and allocate a
    /// `Datum::List(ArgList, ..)` to hold them — a `VecDeque` heap allocation
    /// plus an arena allocation — purely so the very next opcode could
    /// destructure it again. Leaving the arguments in place and pushing this
    /// instead removes both. Nesting is unaffected: the marker occupies the same
    /// stack position the list did, so `foo(a, bar(b))` resolves identically.
    ArgMarker { count: u16, no_ret: bool },
}

impl StackDatum {
    /// Materialize this value against the explicit owner allocator.
    #[inline]
    pub fn into_ref_with(
        self,
        allocator: &mut DatumAllocator,
        bitmap_manager: &mut BitmapManager,
    ) -> DatumRef {
        match self {
            StackDatum::Ref(dr) => dr,
            StackDatum::Void => DatumRef::Void,
            StackDatum::Int(n) => {
                allocator.drain_reclaims(bitmap_manager);
                allocator.alloc_int(n)
            }
            StackDatum::Symbol(s) => {
                allocator.drain_reclaims(bitmap_manager);
                allocator.alloc_symbol(s)
            }
            StackDatum::Float(f) => {
                allocator.drain_reclaims(bitmap_manager);
                allocator
                    .alloc_datum(Datum::Float(f), bitmap_manager)
                    .unwrap()
            }
            // A marker is consumed by the call opcode that follows it and is
            // never a value. Degrade to Void rather than panic if some generic
            // stack reader (debugger display, error unwinding) reaches one.
            StackDatum::ArgMarker { .. } => DatumRef::Void,
        }
    }
}

/// The Lingo operand stack. Primitive values remain inline until an explicit
/// owner-aware consumer asks for a `DatumRef`. Materialization is cached in the
/// slot, so repeated reads preserve one arena identity without requiring
/// interior mutability or an ambient player.
#[derive(Clone, Default)]
pub struct OperandStack {
    items: Vec<StackDatum>,
}

impl OperandStack {
    #[inline]
    pub fn new() -> Self {
        OperandStack { items: Vec::new() }
    }

    // --- Explicit owner-aware DatumRef API ---
    #[inline]
    pub fn push(&mut self, dr: DatumRef) {
        self.items.push(StackDatum::Ref(dr));
    }
    #[inline]
    pub fn pop_ref_with(
        &mut self,
        allocator: &mut DatumAllocator,
        bitmap_manager: &mut BitmapManager,
    ) -> Option<DatumRef> {
        self.items
            .pop()
            .map(|value| value.into_ref_with(allocator, bitmap_manager))
    }
    /// Pop the top entry as a raw `StackDatum` (inline value or ref) WITHOUT
    /// materializing. Inline-aware consumers (arithmetic, compare, jmpifz) use
    /// this so an inline int/float never round-trips through the arena.
    #[inline]
    pub fn pop_value(&mut self) -> Option<StackDatum> {
        self.items.pop()
    }
    #[inline]
    pub fn len(&self) -> usize {
        self.items.len()
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    #[inline]
    pub fn clear(&mut self) {
        self.items.clear();
    }
    #[inline]
    pub fn truncate(&mut self, n: usize) {
        self.items.truncate(n);
    }
    #[inline]
    pub fn swap(&mut self, a: usize, b: usize) {
        self.items.swap(a, b);
    }
    #[inline]
    pub fn last_ref_with(
        &mut self,
        allocator: &mut DatumAllocator,
        bitmap_manager: &mut BitmapManager,
    ) -> Option<DatumRef> {
        self.get_ref_with(self.items.len().checked_sub(1)?, allocator, bitmap_manager)
    }
    #[inline]
    pub fn get_ref_with(
        &mut self,
        i: usize,
        allocator: &mut DatumAllocator,
        bitmap_manager: &mut BitmapManager,
    ) -> Option<DatumRef> {
        let value = self.items.get_mut(i)?;
        if !matches!(value, StackDatum::Ref(_)) {
            let inline = std::mem::replace(value, StackDatum::Void);
            *value = StackDatum::Ref(inline.into_ref_with(allocator, bitmap_manager));
        }
        match value {
            StackDatum::Ref(dr) => Some(dr.clone()),
            _ => unreachable!("materialization guarantees Ref"),
        }
    }
    /// Discard the top `n` entries without materializing them. Dropping a
    /// `Ref` entry decrements its arena refcount exactly as moving it out and
    /// dropping the `DatumRef` would, but inline primitives (the common case
    /// for a discarded expression result) are dropped for free — no `alloc_int`
    /// round-trip just to throw the value away. Used by the `Pop` opcode.
    #[inline]
    pub fn discard(&mut self, n: usize) {
        let new_len = self.items.len().saturating_sub(n);
        self.items.truncate(new_len);
    }
    /// Move the top `n` entries out as owned `DatumRef`s (used by pop_n).
    #[inline]
    pub fn split_off_refs_with(
        &mut self,
        at: usize,
        allocator: &mut DatumAllocator,
        bitmap_manager: &mut BitmapManager,
    ) -> Vec<DatumRef> {
        self.items
            .split_off(at)
            .into_iter()
            .map(|value| value.into_ref_with(allocator, bitmap_manager))
            .collect()
    }
    /// Drain the top `n` entries directly into `buf` (materializing inline values),
    /// removing them from the stack in place. Unlike `split_off_refs`, `drain` does
    /// NOT allocate a transient `Vec` for the removed tail — the arg list is built in
    /// one pass straight into the deque the call opcode consumes. `push_arglist` runs
    /// once per Lingo call (8.1M times in the Habbo preloader), so dropping that extra
    /// per-call allocation matters. `buf` is a (typically pooled) deque, cleared first.
    #[inline]
    pub fn drain_top_into_deque_with(
        &mut self,
        n: usize,
        mut buf: std::collections::VecDeque<DatumRef>,
        allocator: &mut DatumAllocator,
        bitmap_manager: &mut BitmapManager,
    ) -> std::collections::VecDeque<DatumRef> {
        let at = self.items.len() - n;
        buf.clear();
        buf.reserve(n);
        for value in self.items.drain(at..) {
            buf.push_back(value.into_ref_with(allocator, bitmap_manager));
        }
        buf
    }
    /// Materialize a diagnostic snapshot without exposing stack borrows.
    pub fn snapshot_refs_with(
        &mut self,
        allocator: &mut DatumAllocator,
        bitmap_manager: &mut BitmapManager,
    ) -> Vec<DatumRef> {
        (0..self.items.len())
            .map(|i| self.get_ref_with(i, allocator, bitmap_manager).unwrap())
            .collect()
    }

    // --- Inline push fast paths (no Datum/arena) ---
    /// Push an already-formed `StackDatum` (inline value or ref) without
    /// materializing it. The IR runner operates in `StackDatum` terms and shares
    /// this stack with the interpreter, so it needs the untyped push.
    #[inline]
    pub fn push_value(&mut self, v: StackDatum) {
        self.items.push(v);
    }
    #[inline]
    pub fn push_int(&mut self, n: i32) {
        self.items.push(StackDatum::Int(n));
    }
    #[inline]
    pub fn push_float(&mut self, f: f64) {
        self.items.push(StackDatum::Float(f));
    }
    #[inline]
    pub fn push_symbol(&mut self, s: Symbol) {
        self.items.push(StackDatum::Symbol(s));
    }
    #[inline]
    pub fn push_void(&mut self) {
        self.items.push(StackDatum::Void);
    }
}

// #[derive(Clone)]
pub struct Scope {
    pub scope_ref: ScopeRef,
    pub script_ref: CastMemberRef,
    pub receiver: Option<ScriptInstanceRef>,
    pub handler_name_id: u16,
    pub args: Vec<DatumRef>,
    pub bytecode_index: usize,
    /// Handler locals, indexed by DENSE SLOT — the same index the bytecode
    /// carries (`bytecode.obj / multiplier`) and the same one the register IR
    /// uses. This was an `FxHashMap<u16, DatumRef>` keyed by the SPARSE
    /// name-table id, which every call site reached by computing the slot and
    /// then mapping it through `handler.local_name_ids` purely to build a hash
    /// key; indexing by slot removes that step rather than adding one.
    pub locals: Vec<StackDatum>,
    /// Whether each slot has ever been assigned in this invocation.
    ///
    /// Load-bearing for `do`/`eval` only. The old hash map encoded "this frame
    /// has no such local" as key ABSENCE, and `eval.rs`'s resolver relies on
    /// that to fall through to `me` and then globals. A dense vector has every
    /// declared slot present from the start, so without this a declared but
    /// never-assigned local would start shadowing a same-named global —
    /// silently. Written wherever a local is written (strictly cheaper than
    /// the hash insert it replaces) and read only by that resolver.
    pub locals_assigned: Vec<bool>,
    pub loop_return_indices: Vec<usize>,
    pub return_value: DatumRef,
    pub stack: OperandStack,
    pub passed: bool,
    /// Set by the `pass` command: "The pass command branches to the next
    /// location as soon as the command runs. Any Lingo that follows the pass
    /// command in the handler does not run." (Director 11.5 Scripting
    /// Dictionary, `pass`). The bytecode loop ends the handler when it sees
    /// this, which `passed` alone must not do — `passed` is also propagated up
    /// from a nested call to drive event propagation.
    pub stop_requested: bool,
    pub generation: u64,
    /// Cached handler-level instance for get_prop/set_prop (avoids ancestor chain walk per access)
    pub cached_handler_instance: Option<ScriptInstanceRef>,
}

pub struct ScopeResult {
    pub return_value: DatumRef,
    pub passed: bool,
}

impl Scope {
    /// Pop a call's `ArgMarker` and the arguments beneath it, in stack order.
    /// Returns `(args, no_ret)`.
    ///
    /// `None` means the top of stack was not a marker, which would mean the
    /// bytecode ran a call opcode without a preceding `pusharglist`. Callers
    /// report that as a stack error rather than guessing.
    pub fn pop_call_args(
        &mut self,
        allocator: &mut DatumAllocator,
        bitmap_manager: &mut BitmapManager,
    ) -> Option<(Vec<DatumRef>, bool)> {
        let (count, no_ret) = match self.stack.pop_value()? {
            StackDatum::ArgMarker { count, no_ret } => (count as usize, no_ret),
            // Not a marker: put nothing back — the caller errors out. Restoring
            // it would need a push and the frame is being torn down anyway.
            _ => return None,
        };
        if self.stack.len() < count {
            return None;
        }
        Some((self.pop_n(count, allocator, bitmap_manager), no_ret))
    }

    pub fn pop_n(
        &mut self,
        n: usize,
        allocator: &mut DatumAllocator,
        bitmap_manager: &mut BitmapManager,
    ) -> Vec<DatumRef> {
        // Move the top `n` entries out of the stack rather than clone-then-pop.
        // `split_off` transfers ownership of the tail with zero ref-count churn,
        // where the old `to_vec()` + pop loop did 2n ref-count ops plus an extra
        // allocation. `pusharglist`/`pusharglistnoret` (the heaviest opcodes in
        // the Habbo preloader) call this on every Lingo call.
        let split_at = self.stack.len() - n;
        self.stack
            .split_off_refs_with(split_at, allocator, bitmap_manager)
    }

    pub fn default(scope_ref: ScopeRef) -> Scope {
        Scope {
            scope_ref,
            script_ref: INVALID_CAST_MEMBER_REF,
            receiver: None,
            handler_name_id: 0,
            args: vec![],
            bytecode_index: 0,
            locals: Vec::new(),
            locals_assigned: Vec::new(),
            loop_return_indices: vec![],
            return_value: DatumRef::Void,
            stack: OperandStack::new(),
            passed: false,
            stop_requested: false,
            generation: 0,
            cached_handler_instance: None,
        }
    }

    /// Size the local file for a handler. Called once per handler entry; the
    /// scope pool keeps its capacity across reuse, so after warmup this is a
    /// memset rather than an allocation.
    pub fn ensure_locals(&mut self, n_locals: usize) {
        if self.locals.len() < n_locals {
            self.locals.resize(n_locals, StackDatum::Void);
            self.locals_assigned.resize(n_locals, false);
        }
    }

    /// Read a local by slot. Out-of-range reads VOID rather than panicking:
    /// the slot comes from bytecode, and a malformed handler must not be able
    /// to take the player down.
    #[inline]
    pub fn local(&self, slot: usize) -> StackDatum {
        self.locals.get(slot).cloned().unwrap_or(StackDatum::Void)
    }

    /// Write a local by slot, growing the file if the bytecode names a slot
    /// beyond the handler's declared count.
    #[inline]
    pub fn set_local(&mut self, slot: usize, value: StackDatum) {
        if slot >= self.locals.len() {
            self.locals.resize(slot + 1, StackDatum::Void);
            self.locals_assigned.resize(slot + 1, false);
        }
        self.locals[slot] = value;
        self.locals_assigned[slot] = true;
    }

    /// Read an argument by index, VOID past the end. Mirrors `local`, and
    /// keeps malformed bytecode from turning an argument lookup into a panic.
    #[inline]
    pub fn arg(&self, index: usize) -> DatumRef {
        self.args.get(index).cloned().unwrap_or(DatumRef::Void)
    }

    /// Has this slot ever been assigned in this invocation? Only `do`/`eval`
    /// name resolution needs this — see `locals_assigned`.
    #[inline]
    pub fn local_is_assigned(&self, slot: usize) -> bool {
        self.locals_assigned.get(slot).copied().unwrap_or(false)
    }

    pub fn reset(&mut self) {
        // Bump the generation before clearing the slot so a suspended handler
        // can never become valid again after slot reuse.
        self.generation = self
            .generation
            .checked_add(1)
            .expect("scope generation exhausted");
        self.script_ref = INVALID_CAST_MEMBER_REF;
        self.receiver = None;
        self.cached_handler_instance = None;
        self.handler_name_id = 0;
        self.args.clear();
        self.bytecode_index = 0;
        self.locals.clear();
        self.locals_assigned.clear();
        self.loop_return_indices.clear();
        self.return_value = DatumRef::Void;
        self.stack.clear();
        self.passed = false;
        self.stop_requested = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::{
        allocator::DatumAllocatorTrait,
        ownership::OwnerKey,
    };

    fn allocator() -> DatumAllocator {
        DatumAllocator::new(OwnerKey::transitional())
    }

    #[test]
    fn repeated_materialization_reuses_the_cached_handle() {
        let mut allocator = allocator();
        let mut bitmaps = BitmapManager::new();
        let mut stack = OperandStack::new();
        stack.push_float(12.5);

        let first = stack.get_ref_with(0, &mut allocator, &mut bitmaps).unwrap();
        let second = stack.last_ref_with(&mut allocator, &mut bitmaps).unwrap();
        let snapshot = stack.snapshot_refs_with(&mut allocator, &mut bitmaps);

        assert_eq!(first, second);
        assert_eq!(snapshot, vec![first.clone()]);
        assert!(matches!(
            allocator.try_get_datum(&first),
            Some(Datum::Float(value)) if *value == 12.5
        ));
        let next = allocator
            .alloc_datum(Datum::String("next".into()), &mut bitmaps)
            .unwrap();
        assert_eq!(next.unwrap(), first.unwrap() + 1);
    }

    #[test]
    fn inline_materialization_stays_in_its_owner_arena() {
        let mut first = allocator();
        let mut second = allocator();
        let mut first_bitmaps = BitmapManager::new();
        let mut second_bitmaps = BitmapManager::new();
        let mut stack = OperandStack::new();
        stack.push_int(10_000);
        let second_reference = second
            .alloc_datum(Datum::String("second".into()), &mut second_bitmaps)
            .unwrap();

        let reference = stack
            .pop_ref_with(&mut first, &mut first_bitmaps)
            .unwrap();
        let id = reference.unwrap();
        assert_eq!(id, second_reference.unwrap());
        assert!(matches!(
            first.try_get_datum(&reference),
            Some(Datum::Int(value)) if *value == 10_000
        ));
        assert!(second.try_get_datum(&reference).is_none());

        drop(reference);
        first.drain_reclaims(&mut first_bitmaps);
        second.drain_reclaims(&mut second_bitmaps);
        assert!(!first.contains_datum(id));
        assert!(matches!(
            second.try_get_datum(&second_reference),
            Some(Datum::String(value)) if value == "second"
        ));
    }

    #[test]
    fn discard_drops_inline_values_without_materializing_them() {
        let mut allocator = allocator();
        let mut bitmaps = BitmapManager::new();
        let before = allocator
            .alloc_datum(Datum::String("before".into()), &mut bitmaps)
            .unwrap();
        let mut stack = OperandStack::new();
        stack.push_float(3.25);
        stack.discard(1);
        let after = allocator
            .alloc_datum(Datum::String("after".into()), &mut bitmaps)
            .unwrap();

        assert_eq!(after.unwrap(), before.unwrap() + 1);
    }
}
