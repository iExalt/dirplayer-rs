use super::{allocator::ScriptInstanceAllocatorTrait, ownership::OwnerToken, script::ScriptInstanceId};

#[derive(Debug)]
pub struct ScriptInstanceRef(ScriptInstanceId, *mut u32, OwnerToken);

impl ScriptInstanceRef {
    #[inline]
    pub(crate) fn from_id(id: ScriptInstanceId, ref_count: *mut u32, owner: OwnerToken) -> Self {
        let val = id.into();
        if !owner.retain_handle() {
            return Self(val, ref_count, owner);
        }
        unsafe {
            let mut_ref = &mut *ref_count;
            *mut_ref += 1;
        }
        Self(val, ref_count, owner)
    }

    #[inline]
    pub fn id(&self) -> ScriptInstanceId {
        self.0
    }

    #[inline]
    pub(crate) fn owner(&self) -> &OwnerToken { &self.2 }

    #[inline]
    pub(crate) fn ref_count_ptr(&self) -> *mut u32 { self.1 }
}

impl std::ops::Deref for ScriptInstanceRef {
    type Target = ScriptInstanceId;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Clone for ScriptInstanceRef {
    fn clone(&self) -> Self {
        Self::from_id(self.0, self.1, self.2.clone())
    }
}

impl Drop for ScriptInstanceRef {
    #[inline]
    fn drop(&mut self) {
        if !self.2.is_arena_live() {
            return;
        }
        let rc = unsafe { &mut *self.1 };
        if *rc == 0 {
            return;
        }
        *rc -= 1;
        if *rc == 0 {
            self.2.enqueue(super::ownership::ReclaimKind::ScriptInstance(self.0));
        }
    }
}

impl std::fmt::Display for ScriptInstanceRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
