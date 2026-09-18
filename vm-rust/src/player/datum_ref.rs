use std::fmt::Display;

use super::{allocator::DatumAllocatorTrait, ownership::OwnerToken};

pub type DatumId = usize;

pub(crate) struct DatumHandle {
    id: DatumId,
    ref_count: *mut u32,
    owner: OwnerToken,
}

pub enum DatumRef {
    Void,
    Ref(DatumHandle),
}

impl DatumRef {
    #[inline]
    pub(crate) fn from_id(id: DatumId, ref_count: *mut u32, owner: OwnerToken) -> DatumRef {
        if id != 0 && owner.retain_handle() {
            let mut_ref = unsafe { &mut *ref_count };
            if *mut_ref != u32::MAX {
                *mut_ref += 1;
            }
            DatumRef::Ref(DatumHandle {
                id,
                ref_count,
                owner,
            })
        } else {
            DatumRef::Void
        }
    }

    #[inline]
    pub(crate) fn from_allocated(id: DatumId, ref_count: *mut u32, owner: OwnerToken) -> DatumRef {
        DatumRef::Ref(DatumHandle {
            id,
            ref_count,
            owner,
        })
    }

    #[inline]
    pub(crate) fn owner(&self) -> Option<&OwnerToken> {
        match self {
            DatumRef::Ref(handle) => Some(&handle.owner),
            DatumRef::Void => None,
        }
    }

    #[inline]
    pub(crate) fn ref_count_ptr(&self) -> Option<*mut u32> {
        match self {
            DatumRef::Ref(handle) => Some(handle.ref_count),
            DatumRef::Void => None,
        }
    }

    #[inline]
    pub fn unwrap(&self) -> DatumId {
        match self {
            DatumRef::Void => 0,
            DatumRef::Ref(handle) => handle.id,
        }
    }
}

impl PartialEq for DatumRef {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (DatumRef::Void, DatumRef::Void) => true,
            (DatumRef::Ref(handle), DatumRef::Void) => handle.id == 0,
            (DatumRef::Void, DatumRef::Ref(handle)) => handle.id == 0,
            (DatumRef::Ref(handle1), DatumRef::Ref(handle2)) => {
                handle1.id == handle2.id && handle1.owner.same_identity(&handle2.owner)
            }
            _ => false,
        }
    }
}

impl core::fmt::Debug for DatumRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DatumRef::Void => write!(f, "DatumRef(Void)"),
            DatumRef::Ref(handle) => write!(f, "DatumRef({})", handle.id),
        }
    }
}

impl Clone for DatumRef {
    fn clone(&self) -> Self {
        match self {
            DatumRef::Void => DatumRef::Void,
            DatumRef::Ref(handle) => {
                if handle.owner.is_arena_live() {
                    DatumRef::from_id(handle.id, handle.ref_count, handle.owner.clone())
                } else {
                    DatumRef::from_allocated(handle.id, handle.ref_count, handle.owner.clone())
                }
            }
        }
    }
}

impl Drop for DatumRef {
    #[inline]
    fn drop(&mut self) {
        if let DatumRef::Ref(handle) = self {
            if !handle.owner.is_arena_live() {
                return;
            }
            let rc = unsafe { &mut *handle.ref_count };
            if *rc == u32::MAX {
                return;
            }
            if *rc == 0 {
                return;
            }
            *rc -= 1;
            if *rc == 0 {
                handle
                    .owner
                    .enqueue(super::ownership::ReclaimKind::Datum(handle.id));
            }
        }
    }
}

impl Display for DatumRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DatumRef::Void => write!(f, "DatumRef(Void)"),
            DatumRef::Ref(handle) => write!(f, "DatumRef({})", handle.id),
        }
    }
}
