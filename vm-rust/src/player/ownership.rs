//! Ownership metadata for arena-backed runtime handles.
//!
//! The token deliberately contains no pointer to a player or allocator.  It is
//! only a lifetime/epoch capability and a small deferred-reclamation queue;
//! arena and bitmap effects are applied by the owning allocator.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct OwnerKey {
    pub session: u64,
    pub player: u64,
    pub generation: u64,
}

impl OwnerKey {
    /// Transitional constructor used until RuntimeSession supplies stable
    /// session/player IDs. Rc identity remains authoritative during this
    /// transition, so equal keys never make handles interchangeable.
    pub const fn transitional() -> Self {
        Self {
            session: 0,
            player: 0,
            generation: 0,
        }
    }

    pub const fn checked_next_generation(self) -> Option<Self> {
        match self.generation.checked_add(1) {
            Some(generation) => Some(Self { generation, ..self }),
            None => None,
        }
    }

    pub const fn next_generation(self) -> Self {
        match self.checked_next_generation() {
            Some(next) => next,
            None => panic!("owner generation exhausted"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OwnerState {
    Live,
    Resetting,
    ArenaDead,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReclaimKind {
    Datum(usize),
    ScriptInstance(u32),
}

#[derive(Clone, Debug)]
pub struct ReclaimItem {
    pub owner: OwnerKey,
    pub kind: ReclaimKind,
}

#[derive(Debug)]
struct OwnerStateInner {
    key: OwnerKey,
    state: Cell<OwnerState>,
    pending: RefCell<Vec<ReclaimItem>>,
}

/// A cheap, cloneable capability carried by every arena handle.
#[derive(Clone, Debug)]
pub struct OwnerToken(Rc<OwnerStateInner>);

impl OwnerToken {
    pub fn new(key: OwnerKey) -> Self {
        Self(Rc::new(OwnerStateInner {
            key,
            state: Cell::new(OwnerState::Live),
            pending: RefCell::new(Vec::new()),
        }))
    }

    pub fn transitional() -> Self {
        Self::new(OwnerKey::transitional())
    }

    #[inline]
    pub fn key(&self) -> OwnerKey {
        self.0.key
    }

    #[inline]
    pub fn same_identity(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }

    #[inline]
    pub fn is_arena_live(&self) -> bool {
        self.0.state.get() == OwnerState::Live
    }

    pub(crate) fn begin_reset(&self) {
        debug_assert_eq!(self.0.state.get(), OwnerState::Live);
        self.0.state.set(OwnerState::Resetting);
    }

    /// Mark the capability dead before its arena fields are dropped.
    pub(crate) fn mark_arena_dead(&self) {
        self.0.state.set(OwnerState::ArenaDead);
    }

    /// Returns false for stale/resetting handles. Clone and constructors use
    /// this before touching a raw arena refcount pointer.
    #[inline]
    pub(crate) fn retain_handle(&self) -> bool {
        self.is_arena_live()
    }

    pub(crate) fn enqueue(&self, kind: ReclaimKind) {
        self.0.pending.borrow_mut().push(ReclaimItem {
            owner: self.key(),
            kind,
        });
    }

    pub(crate) fn take_pending(&self) -> Vec<ReclaimItem> {
        std::mem::take(&mut *self.0.pending.borrow_mut())
    }

    pub(crate) fn discard_pending(&self) {
        self.0.pending.borrow_mut().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_stronger_than_equal_transitional_keys() {
        let a = OwnerToken::transitional();
        let b = OwnerToken::transitional();
        assert_eq!(a.key(), b.key());
        assert!(!a.same_identity(&b));
    }

    #[test]
    fn reset_rejects_new_handles_and_invalidates_queue() {
        let owner = OwnerToken::transitional();
        assert!(owner.retain_handle());
        owner.enqueue(ReclaimKind::Datum(3));
        owner.begin_reset();
        assert!(!owner.retain_handle());
        assert_eq!(owner.take_pending().len(), 1);
        owner.mark_arena_dead();
        assert!(!owner.is_arena_live());
    }

    #[test]
    fn independent_owners_can_run_on_separate_threads() {
        let first = std::thread::spawn(|| {
            let owner = OwnerToken::transitional();
            owner.retain_handle() && owner.is_arena_live()
        });
        let second = std::thread::spawn(|| {
            let owner = OwnerToken::transitional();
            owner.retain_handle() && owner.is_arena_live()
        });
        assert!(first.join().unwrap());
        assert!(second.join().unwrap());
    }

    #[test]
    fn checked_owner_generation_rejects_exhaustion_without_wrapping() {
        let key = OwnerKey {
            session: 1,
            player: 2,
            generation: u64::MAX,
        };
        assert!(key.checked_next_generation().is_none());
    }
}
