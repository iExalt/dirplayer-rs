use std::cell::UnsafeCell;

use fxhash::FxHashMap;
use log::{debug, warn};

use crate::{director::lingo::datum::Datum, player::symbols::symbol::Symbol};

use super::{
    datum_ref::{DatumId, DatumRef},
    ownership::{OwnerKey, OwnerToken, ReclaimKind},
    script::{ScriptInstance, ScriptInstanceId},
    script_ref::ScriptInstanceRef,
    ScriptError, ScriptErrorCode,
};

const ARENA_CHUNK_SIZE: usize = 4096;

const INT_POOL_MIN: i32 = -1024;
const INT_POOL_MAX: i32 = 4096;
const INT_POOL_SIZE: usize = (INT_POOL_MAX - INT_POOL_MIN + 1) as usize; // 5121

pub struct Arena<T> {
    chunks: Vec<Box<[Option<T>]>>,
    free_list: Vec<usize>,
    count: usize,
    next_slot: usize,
}

impl<T> Arena<T> {
    pub fn new() -> Self {
        Arena {
            chunks: Vec::new(),
            free_list: Vec::new(),
            count: 0,
            next_slot: 0,
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        let num_chunks = (capacity + ARENA_CHUNK_SIZE - 1) / ARENA_CHUNK_SIZE;
        let mut chunks = Vec::with_capacity(num_chunks);
        for _ in 0..num_chunks {
            chunks.push(Self::new_chunk());
        }
        Arena {
            chunks,
            free_list: Vec::with_capacity(capacity),
            count: 0,
            next_slot: 0,
        }
    }

    fn new_chunk() -> Box<[Option<T>]> {
        let mut chunk = Vec::with_capacity(ARENA_CHUNK_SIZE);
        chunk.resize_with(ARENA_CHUNK_SIZE, || None);
        chunk.into_boxed_slice()
    }

    fn ensure_chunk(&mut self, chunk_idx: usize) {
        while self.chunks.len() <= chunk_idx {
            self.chunks.push(Self::new_chunk());
        }
    }

    #[inline]
    pub(crate) fn alloc(&mut self, value: T) -> usize {
        self.count += 1;
        if let Some(idx) = self.free_list.pop() {
            self.chunks[idx / ARENA_CHUNK_SIZE][idx % ARENA_CHUNK_SIZE] = Some(value);
            idx + 1
        } else {
            let idx = self.next_slot;
            self.ensure_chunk(idx / ARENA_CHUNK_SIZE);
            self.chunks[idx / ARENA_CHUNK_SIZE][idx % ARENA_CHUNK_SIZE] = Some(value);
            self.next_slot += 1;
            idx + 1
        }
    }

    pub(crate) fn insert_at(&mut self, id: usize, value: T) {
        assert!(id > 0, "arena ids are one-based");
        let idx = id - 1;
        self.ensure_chunk(idx / ARENA_CHUNK_SIZE);
        let chunk_idx = idx / ARENA_CHUNK_SIZE;
        let slot_idx = idx % ARENA_CHUNK_SIZE;
        // Use take() to safely drop the old value (if any) before inserting
        let was_empty = self.chunks[chunk_idx][slot_idx].take().is_none();
        self.chunks[chunk_idx][slot_idx] = Some(value);
        if was_empty {
            self.count += 1;
        }
        if idx >= self.next_slot {
            self.next_slot = idx + 1;
        }
    }

    #[inline]
    pub(crate) fn remove(&mut self, id: usize) -> Option<T> {
        if id == 0 {
            return None;
        }
        let idx = id - 1;
        let chunk_idx = idx / ARENA_CHUNK_SIZE;
        if chunk_idx < self.chunks.len() {
            let slot_idx = idx % ARENA_CHUNK_SIZE;
            if let Some(value) = self.chunks[chunk_idx][slot_idx].take() {
                self.free_list.push(idx);
                self.count -= 1;
                Some(value)
            } else {
                None
            }
        } else {
            None
        }
    }

    #[inline]
    pub fn get(&self, id: usize) -> Option<&T> {
        if id == 0 {
            return None;
        }
        let idx = id - 1;
        let chunk_idx = idx / ARENA_CHUNK_SIZE;
        if chunk_idx < self.chunks.len() {
            self.chunks[chunk_idx][idx % ARENA_CHUNK_SIZE].as_ref()
        } else {
            None
        }
    }

    #[inline]
    pub(crate) fn get_mut(&mut self, id: usize) -> Option<&mut T> {
        if id == 0 {
            return None;
        }
        let idx = id - 1;
        let chunk_idx = idx / ARENA_CHUNK_SIZE;
        if chunk_idx < self.chunks.len() {
            self.chunks[chunk_idx][idx % ARENA_CHUNK_SIZE].as_mut()
        } else {
            None
        }
    }

    #[inline]
    pub fn contains(&self, id: usize) -> bool {
        if id == 0 {
            return false;
        }
        let idx = id - 1;
        let chunk_idx = idx / ARENA_CHUNK_SIZE;
        chunk_idx < self.chunks.len()
            && self.chunks[chunk_idx][idx % ARENA_CHUNK_SIZE].is_some()
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn iter(&self) -> impl Iterator<Item = (usize, &T)> {
        let next_slot = self.next_slot;
        (0..next_slot).filter_map(move |idx| {
            let chunk_idx = idx / ARENA_CHUNK_SIZE;
            let slot_idx = idx % ARENA_CHUNK_SIZE;
            if chunk_idx < self.chunks.len() {
                self.chunks[chunk_idx][slot_idx].as_ref().map(|v| (idx + 1, v))
            } else {
                None
            }
        })
    }

    pub(crate) fn clear(&mut self) {
        self.chunks.clear();
        self.free_list.clear();
        self.count = 0;
        self.next_slot = 0;
    }

    pub(crate) fn clear_individually_reverse(&mut self) {
        for chunk_idx in (0..self.chunks.len()).rev() {
            for slot_idx in (0..ARENA_CHUNK_SIZE).rev() {
                // Use take() so the slot is set to None BEFORE the value is
                // dropped. This ensures re-entrant contains() checks during
                // drop cascades correctly see the slot as empty.
                drop(self.chunks[chunk_idx][slot_idx].take());
            }
        }
        self.free_list.clear();
        self.count = 0;
        self.next_slot = 0;
    }

    pub(crate) fn clear_individually(&mut self) {
        for chunk_idx in 0..self.chunks.len() {
            for slot_idx in 0..ARENA_CHUNK_SIZE {
                drop(self.chunks[chunk_idx][slot_idx].take());
            }
        }
        self.free_list.clear();
        self.count = 0;
        self.next_slot = 0;
    }
}

pub struct DatumRefEntry {
    pub id: DatumId,
    pub ref_count: UnsafeCell<u32>,
    pub datum: Datum,
}

pub struct ScriptInstanceRefEntry {
    pub id: ScriptInstanceId,
    pub ref_count: UnsafeCell<u32>,
    pub script_instance: ScriptInstance,
}

pub trait ResetableAllocator {
    fn reset(&mut self, bitmap_manager: &mut crate::player::bitmap::manager::BitmapManager)
        -> OwnerToken;
}

pub(crate) trait DatumAllocatorTrait {
    fn alloc_datum(
        &mut self,
        datum: Datum,
        bitmap_manager: &mut crate::player::bitmap::manager::BitmapManager,
    ) -> Result<DatumRef, ScriptError>;
    fn get_datum(&self, id: &DatumRef) -> &Datum;
    fn get_datum_mut(&mut self, id: &DatumRef) -> &mut Datum;
}

pub trait ScriptInstanceAllocatorTrait {
    fn alloc_script_instance(&mut self, script_instance: ScriptInstance) -> ScriptInstanceRef;
    fn get_script_instance(&self, instance_ref: &ScriptInstanceRef) -> &ScriptInstance;
    fn get_script_instance_opt(&self, instance_ref: &ScriptInstanceRef) -> Option<&ScriptInstance>;
    fn get_script_instance_mut(&mut self, instance_ref: &ScriptInstanceRef) -> &mut ScriptInstance;
}

pub struct DatumAllocator {
    owner: OwnerToken,
    datums: Arena<DatumRefEntry>,
    script_instances: Arena<ScriptInstanceRefEntry>,
    script_instance_counter: ScriptInstanceId,
    void_datum: Datum,
    pub int_alloc_count: usize,
    pub int_dealloc_count: usize,
    pub snapshot_max_id: usize,
    int_pool_ids: [DatumId; INT_POOL_SIZE],
    /// Cached ref_count pointers for the pooled ints, parallel to
    /// `int_pool_ids`. Pooled entries are immortal and never move in the arena,
    /// so caching the pointer lets `alloc_datum` build the `DatumRef` without an
    /// arena `datums.get(id)` lookup — that lookup is cheap natively but a large
    /// fraction of per-push cost under WASM.
    int_pool_refs: [*mut u32; INT_POOL_SIZE],
    /// Symbol -> (datum id, ref_count pointer). Same idea as the int pool: skip
    /// the arena lookup when re-pushing an already-interned symbol.
    symbol_pool: FxHashMap<Symbol, (DatumId, *mut u32)>,
    #[cfg(test)]
    reset_fault: bool,
}

const MAX_SCRIPT_INSTANCE_ID: ScriptInstanceId = 0xFFFFFF;

impl DatumAllocator {
    pub fn default() -> Self {
        Self::new(OwnerKey::transitional())
    }

    pub fn new(key: OwnerKey) -> Self {
        Self::from_owner(OwnerToken::new(key))
    }

    pub(crate) fn from_owner(owner: OwnerToken) -> Self {
        let mut alloc = DatumAllocator {
            owner,
            datums: Arena::with_capacity(4096),
            script_instances: Arena::new(),
            script_instance_counter: 1,
            void_datum: Datum::Void,
            int_alloc_count: 0,
            int_dealloc_count: 0,
            snapshot_max_id: 0,
            int_pool_ids: [0; INT_POOL_SIZE],
            int_pool_refs: [std::ptr::null_mut(); INT_POOL_SIZE],
            symbol_pool: FxHashMap::default(),
            #[cfg(test)]
            reset_fault: false,
        };
        alloc.init_int_pool();
        alloc
    }

    #[inline]
    pub fn owner_token(&self) -> OwnerToken { self.owner.clone() }

    /// Drain deferred drops against this allocator. The queue is owner-local;
    /// the key check protects against accidental transfer between epochs.
    pub fn drain_reclaims(
        &mut self,
        bitmap_manager: &mut crate::player::bitmap::manager::BitmapManager,
    ) {
        loop {
            let pending = self.owner.take_pending();
            if pending.is_empty() {
                break;
            }
            for item in pending {
                if item.owner != self.owner.key() {
                    continue;
                }
                match item.kind {
                    ReclaimKind::Datum(id) => {
                        if self
                            .datums
                            .get(id)
                            .map_or(false, |entry| unsafe { *entry.ref_count.get() == 0 })
                        {
                            if let Some(bitmap) = self.dealloc_datum(id) {
                                let _ = bitmap_manager.decref_ephemeral_handle(&bitmap);
                            }
                        }
                    }
                    ReclaimKind::ScriptInstance(id) => {
                        if self
                            .script_instances
                            .get(id as usize)
                            .map_or(false, |entry| unsafe { *entry.ref_count.get() == 0 })
                        {
                            self.dealloc_script_instance(id);
                        }
                    }
                }
            }
        }
    }

    fn release_remaining_bitmap_refs(
        &self,
        bitmap_manager: &mut crate::player::bitmap::manager::BitmapManager,
    ) {
        for (_, entry) in self.datums.iter() {
            if let Datum::BitmapRef(bitmap) = &entry.datum {
                let _ = bitmap_manager.decref_ephemeral_handle(bitmap);
            }
        }
    }

    fn valid_datum_ref(&self, reference: &DatumRef) -> Option<DatumId> {
        let DatumRef::Ref(_) = reference else { return None };
        let Some(owner) = reference.owner() else { return None };
        let Some(ptr) = reference.ref_count_ptr() else { return None };
        if !owner.same_identity(&self.owner) || !owner.is_arena_live() {
            return None;
        }
        let id = reference.unwrap();
        let entry = self.datums.get(id)?;
        if entry.ref_count.get() != ptr || entry.id != id {
            return None;
        }
        Some(id)
    }

    pub(crate) fn try_get_datum(&self, reference: &DatumRef) -> Option<&Datum> {
        let id = self.valid_datum_ref(reference)?;
        Some(&self.datums.get(id)?.datum)
    }

    pub(crate) fn try_get_datum_mut(&mut self, reference: &DatumRef) -> Option<&mut Datum> {
        let id = self.valid_datum_ref(reference)?;
        Some(&mut self.datums.get_mut(id)?.datum)
    }

    /// Replace a live, non-pooled vector without exposing a mutable Datum to
    /// callers that could install a resource-bearing variant.
    pub(crate) fn replace_vector(
        &mut self,
        reference: &DatumRef,
        values: [f64; 3],
    ) -> Result<(), ScriptError> {
        let id = self.valid_datum_ref(reference).ok_or_else(|| {
            ScriptError::new_code(
                ScriptErrorCode::InvalidReference,
                "foreign or stale datum reference".to_string(),
            )
        })?;
        let entry = self.datums.get(id).ok_or_else(|| {
            ScriptError::new_code(
                ScriptErrorCode::InvalidReference,
                "validated datum disappeared".to_string(),
            )
        })?;
        if unsafe { *entry.ref_count.get() == u32::MAX } {
            return Err(ScriptError::new_code(
                ScriptErrorCode::InvalidReference,
                "cannot replace pooled datum".to_string(),
            ));
        }
        if !matches!(entry.datum, Datum::Vector(_)) {
            return Err(ScriptError::new_code(
                ScriptErrorCode::InvalidReference,
                "expected Vector datum".to_string(),
            ));
        }
        self.datums
            .get_mut(id)
            .ok_or_else(|| {
                ScriptError::new_code(
                    ScriptErrorCode::InvalidReference,
                    "validated datum disappeared".to_string(),
                )
            })?
            .datum = Datum::Vector(values);
        Ok(())
    }

    /// Replace a live, non-pooled transform without exposing a mutable Datum
    /// to callers that could install a resource-bearing variant.
    pub(crate) fn replace_transform3d(
        &mut self,
        reference: &DatumRef,
        matrix: [f64; 16],
    ) -> Result<(), ScriptError> {
        let id = self.valid_datum_ref(reference).ok_or_else(|| {
            ScriptError::new_code(
                ScriptErrorCode::InvalidReference,
                "foreign or stale datum reference".to_string(),
            )
        })?;
        let entry = self.datums.get(id).ok_or_else(|| {
            ScriptError::new_code(
                ScriptErrorCode::InvalidReference,
                "validated datum disappeared".to_string(),
            )
        })?;
        if unsafe { *entry.ref_count.get() == u32::MAX } {
            return Err(ScriptError::new_code(
                ScriptErrorCode::InvalidReference,
                "cannot replace pooled datum".to_string(),
            ));
        }
        if !matches!(entry.datum, Datum::Transform3d(_)) {
            return Err(ScriptError::new_code(
                ScriptErrorCode::InvalidReference,
                "expected Transform3d datum".to_string(),
            ));
        }
        self.datums
            .get_mut(id)
            .ok_or_else(|| {
                ScriptError::new_code(
                    ScriptErrorCode::InvalidReference,
                    "validated datum disappeared".to_string(),
                )
            })?
            .datum = Datum::Transform3d(Box::new(matrix));
        Ok(())
    }

    fn valid_script_ref(&self, reference: &ScriptInstanceRef) -> Option<ScriptInstanceId> {
        if !reference.owner().same_identity(&self.owner)
            || !reference.owner().is_arena_live()
        {
            return None;
        }
        let id = reference.id();
        let entry = self.script_instances.get(id as usize)?;
        if entry.ref_count.get() != reference.ref_count_ptr() || entry.id != id {
            return None;
        }
        Some(id)
    }

    fn init_int_pool(&mut self) {
        for i in 0..INT_POOL_SIZE {
            let n = (i as i32) + INT_POOL_MIN;
            let entry = DatumRefEntry {
                id: 0,
                ref_count: UnsafeCell::new(u32::MAX),
                datum: Datum::Int(n),
            };
            let id = self.datums.alloc(entry);
            let entry = self.datums.get_mut(id).unwrap();
            entry.id = id;
            self.int_pool_ids[i] = id;
            self.int_pool_refs[i] = entry.ref_count.get();
        }
    }

    /// Fast path for pushing an int. Pooled values (the common case) return the
    /// cached immortal ref directly, with NO `Datum` enum constructed or moved —
    /// `Datum` is 64 bytes, so building one just to hand to `alloc_datum` is pure
    /// per-push cost under WASM that this avoids.
    #[inline]
    pub fn alloc_int(&mut self, n: i32) -> DatumRef {
        if !self.owner.is_arena_live() {
            return DatumRef::Void;
        }
        if n >= INT_POOL_MIN && n <= INT_POOL_MAX {
            let idx = (n - INT_POOL_MIN) as usize;
            return DatumRef::from_allocated(
                self.int_pool_ids[idx],
                self.int_pool_refs[idx],
                self.owner.clone(),
            );
        }
        self.alloc_datum_core(Datum::Int(n)).unwrap()
    }

    /// Fast path for pushing a symbol: interned symbols return the cached
    /// immortal ref directly (no `Datum` construction).
    #[inline]
    pub fn alloc_symbol(&mut self, sym: Symbol) -> DatumRef {
        if !self.owner.is_arena_live() {
            return DatumRef::Void;
        }
        if let Some(&(id, rc)) = self.symbol_pool.get(&sym) {
            return DatumRef::from_allocated(id, rc, self.owner.clone());
        }
        self.alloc_datum_core(Datum::Symbol(sym)).unwrap()
    }

    #[cfg(test)]
    fn inject_reset_unwind(&mut self) {
        self.reset_fault = true;
    }

    pub fn contains_datum(&self, id: DatumId) -> bool {
        self.datums.contains(id)
    }

    pub(crate) fn iter_datums(&self) -> impl Iterator<Item = (DatumId, &DatumRefEntry)> {
        self.datums.iter()
    }

    pub(crate) fn get_datum_entry(&self, id: DatumId) -> Option<&DatumRefEntry> {
        self.datums.get(id)
    }

    pub(crate) fn iter_script_instances(
        &self,
    ) -> impl Iterator<Item = (usize, &ScriptInstanceRefEntry)> {
        self.script_instances.iter()
    }

    pub fn get_free_script_instance_id(&self) -> ScriptInstanceId {
        if self.script_instance_count() >= MAX_SCRIPT_INSTANCE_ID as usize {
            panic!("Script instance limit reached");
        }
        if !self.script_instances.contains(self.script_instance_counter as usize) {
            self.script_instance_counter
        } else if self.script_instance_counter + 1 < MAX_SCRIPT_INSTANCE_ID
            && !self
                .script_instances
                .contains((self.script_instance_counter + 1) as usize)
        {
            self.script_instance_counter + 1
        } else {
            warn!("Script instance id overflow. Searching for free id...");
            let first_free_id = (1..MAX_SCRIPT_INSTANCE_ID)
                .find(|id| !self.script_instances.contains(*id as usize));
            if let Some(id) = first_free_id {
                id
            } else {
                panic!("Failed to find free script instance id");
            }
        }
    }

    pub fn script_instance_count(&self) -> usize {
        self.script_instances.len()
    }

    pub fn datum_count(&self) -> usize {
        self.datums.len()
    }

    pub fn datum_type_stats(&self) -> String {
        let mut counts: std::collections::HashMap<String, (usize, usize)> = std::collections::HashMap::new();
        // Int-specific tracking
        let mut int_rc_dist: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
        let mut int_value_dist: std::collections::HashMap<i32, usize> = std::collections::HashMap::new();
        let mut int_samples: Vec<(usize, i32, u32)> = Vec::new(); // (id, value, rc)

        for (id, entry) in self.datums.iter() {
            let type_name = entry.datum.type_str().to_string();
            let rc = unsafe { *entry.ref_count.get() };
            let (count, total_rc) = counts.entry(type_name).or_insert((0, 0));
            *count += 1;
            *total_rc += rc as usize;

            // Track Int datum details
            if let Datum::Int(val) = &entry.datum {
                *int_rc_dist.entry(rc).or_insert(0) += 1;
                *int_value_dist.entry(*val).or_insert(0) += 1;
                if int_samples.len() < 20 {
                    int_samples.push((id, *val, rc));
                }
            }
        }
        let mut sorted: Vec<_> = counts.into_iter().collect();
        sorted.sort_by(|a, b| b.1.0.cmp(&a.1.0));
        let mut result = format!("Live datums: {}\n", self.datums.len());
        for (type_name, (count, total_rc)) in &sorted {
            result.push_str(&format!("  {}: {} (total rc: {})\n", type_name, count, total_rc));
        }
        result.push_str(&format!("Free list: {}\n", self.datums.free_list.len()));

        // Int ref count distribution
        let mut rc_sorted: Vec<_> = int_rc_dist.into_iter().collect();
        rc_sorted.sort_by_key(|&(rc, _)| rc);
        result.push_str("Int rc distribution:\n");
        for (rc, count) in &rc_sorted {
            result.push_str(&format!("  rc={}: {}\n", rc, count));
        }

        // Int value distribution (top 15 most common values)
        let mut val_sorted: Vec<_> = int_value_dist.into_iter().collect();
        val_sorted.sort_by(|a, b| b.1.cmp(&a.1));
        result.push_str("Int value distribution (top 15):\n");
        for (val, count) in val_sorted.iter().take(15) {
            result.push_str(&format!("  val={}: {}\n", val, count));
        }

        // Int alloc/dealloc counters
        result.push_str(&format!("Int allocs: {}, deallocs: {}, delta: {}\n",
            self.int_alloc_count, self.int_dealloc_count,
            self.int_alloc_count as i64 - self.int_dealloc_count as i64));

        // Show new Int datums since snapshot (if set)
        if self.snapshot_max_id > 0 {
            let mut new_ints = 0;
            let mut new_int_samples: Vec<(usize, i32, u32)> = Vec::new();
            for (id, entry) in self.datums.iter() {
                if id > self.snapshot_max_id {
                    if let Datum::Int(val) = &entry.datum {
                        let rc = unsafe { *entry.ref_count.get() };
                        new_ints += 1;
                        if new_int_samples.len() < 20 {
                            new_int_samples.push((id, *val, rc));
                        }
                    }
                }
            }
            result.push_str(&format!("New Int datums since snapshot (id>{}): {}\n",
                self.snapshot_max_id, new_ints));
            result.push_str("New Int samples:\n");
            for (id, val, rc) in &new_int_samples {
                result.push_str(&format!("  #{}: val={}, rc={}\n", id, val, rc));
            }
        }

        result
    }

    pub fn take_datum_snapshot(&mut self) {
        self.snapshot_max_id = self.datums.next_slot;
        self.int_alloc_count = 0;
        self.int_dealloc_count = 0;
    }

    /// Free arena slot `id`. Returns the ephemeral `BitmapId` (if any) that
    /// the freed entry was holding, so the caller can run
    /// `bitmap_manager.decref_ephemeral` after returning — keeps the bitmap
    /// manager hop outside of the allocator's own borrow.
    fn dealloc_datum(
        &mut self,
        id: DatumId,
    ) -> Option<crate::player::bitmap::manager::BitmapHandle> {
        let mut bitmap_to_decref = None;
        if let Some(entry) = self.datums.get(id) {
            if unsafe { *entry.ref_count.get() } == u32::MAX {
                return None; // Pooled/immortal entry, never free
            }
            match &entry.datum {
                Datum::Int(_) => {
                    self.int_dealloc_count += 1;
                }
                Datum::BitmapRef(bm_ref) => {
                    bitmap_to_decref = Some(bm_ref.clone());
                }
                _ => {}
            }
        }
        self.datums.remove(id);
        bitmap_to_decref
    }

    fn dealloc_script_instance(&mut self, id: ScriptInstanceId) {
        self.script_instances.remove(id as usize);
    }

    pub fn get_datum_ref(&self, id: DatumId) -> Option<DatumRef> {
        if !self.owner.is_arena_live() {
            return None;
        }
        if let Some(entry) = self.datums.get(id) {
            match DatumRef::from_id(id, entry.ref_count.get(), self.owner.clone()) {
                DatumRef::Void => None,
                reference => Some(reference),
            }
        } else {
            None
        }
    }

    pub fn get_script_instance_ref(&self, id: ScriptInstanceId) -> Option<ScriptInstanceRef> {
        if !self.owner.is_arena_live() {
            return None;
        }
        if let Some(entry) = self.script_instances.get(id as usize) {
            Some(ScriptInstanceRef::from_id(id, entry.ref_count.get(), self.owner.clone()))
        } else {
            None
        }
    }

    pub(crate) fn get_script_instance_entry(
        &self,
        id: ScriptInstanceId,
    ) -> Option<&ScriptInstanceRefEntry> {
        self.script_instances.get(id as usize)
    }

    pub(crate) fn get_script_instance_entry_mut(
        &mut self,
        id: ScriptInstanceId,
    ) -> Option<&mut ScriptInstanceRefEntry> {
        self.script_instances.get_mut(id as usize)
    }

    #[inline]
    fn alloc_datum_core(&mut self, datum: Datum) -> Result<DatumRef, ScriptError> {
        if !self.owner.is_arena_live() {
            return Err(ScriptError::new("allocator owner is not live".to_string()));
        }
        if datum.is_void() {
            return Ok(DatumRef::Void);
        }

        // Return pooled entry for common int values (no arena lookup — the
        // ref_count pointer is cached and stable for immortal pooled entries).
        if let Datum::Int(n) = &datum {
            if *n >= INT_POOL_MIN && *n <= INT_POOL_MAX {
                let pool_idx = (*n - INT_POOL_MIN) as usize;
                return Ok(DatumRef::from_allocated(
                    self.int_pool_ids[pool_idx],
                    self.int_pool_refs[pool_idx],
                    self.owner.clone(),
                ));
            }
        }

        // Intern symbols: same symbol returns the same pooled entry (cached
        // ref_count pointer avoids the arena lookup on re-push).
        if let Datum::Symbol(s) = &datum {
            if let Some(&(id, rc)) = self.symbol_pool.get(s) {
                return Ok(DatumRef::from_allocated(id, rc, self.owner.clone()));
            }
            // First time seeing this symbol — allocate and register.
            let key = s.clone();
            let entry = DatumRefEntry {
                id: 0,
                ref_count: UnsafeCell::new(u32::MAX),
                datum,
            };
            let id = self.datums.alloc(entry);
            let entry = self.datums.get_mut(id).unwrap();
            entry.id = id;
            let rc = entry.ref_count.get();
            self.symbol_pool.insert(key, (id, rc));
            return Ok(DatumRef::from_allocated(id, rc, self.owner.clone()));
        }

        let is_int = matches!(&datum, Datum::Int(_));
        // Capture the bitmap ref (if any) BEFORE moving `datum` into the
        // entry — we incref ephemeral bitmaps so they survive as long as at
        // least one arena entry references them. Cast-member-owned bitmaps
        // aren't in `ephemeral_refs` so the incref is a no-op for them.
        let entry = DatumRefEntry {
            id: 0,
            ref_count: UnsafeCell::new(1), // Start at 1 to avoid the extra increment in from_id
            datum,
        };
        let id = self.datums.alloc(entry);
        let entry = self.datums.get_mut(id).unwrap();
        entry.id = id;
        let ref_count_ptr = entry.ref_count.get();
        if is_int {
            self.int_alloc_count += 1;
        }
        Ok(DatumRef::from_allocated(id, ref_count_ptr, self.owner.clone()))
    }

}

impl DatumAllocatorTrait for DatumAllocator {
    #[inline]
    fn alloc_datum(
        &mut self,
        datum: Datum,
        bitmap_manager: &mut crate::player::bitmap::manager::BitmapManager,
    ) -> Result<DatumRef, ScriptError> {
        let bitmap_to_incref = match &datum {
            Datum::BitmapRef(bitmap) => Some(bitmap.clone()),
            _ => None,
        };
        if let Some(bitmap) = &bitmap_to_incref {
            bitmap_manager
                .get_bitmap_handle(bitmap)
                .ok_or_else(|| {
                    ScriptError::new_code(
                        ScriptErrorCode::InvalidReference,
                        "invalid bitmap handle".to_string(),
                    )
                })?;
        }
        if let Some(bitmap) = &bitmap_to_incref {
            bitmap_manager
                .incref_ephemeral_handle(bitmap)
                .map_err(|error| ScriptError::new(format!("invalid bitmap handle: {error:?}")))?;
        }
        match self.alloc_datum_core(datum) {
            Ok(reference) => Ok(reference),
            Err(error) => {
                if let Some(bitmap) = &bitmap_to_incref {
                    let _ = bitmap_manager.decref_ephemeral_handle(bitmap);
                }
                Err(error)
            }
        }
    }

    #[inline]
    fn get_datum(&self, id: &DatumRef) -> &Datum {
        match id {
            DatumRef::Ref(_) => {
                let datum_id = self
                    .valid_datum_ref(id)
                    .expect("foreign or stale DatumRef");
                &self.datums.get(datum_id).expect("validated datum disappeared").datum
            }
            DatumRef::Void => &Datum::Void,
        }
    }

    #[inline]
    fn get_datum_mut(&mut self, id: &DatumRef) -> &mut Datum {
        match id {
            DatumRef::Ref(_) => {
                let datum_id = self
                    .valid_datum_ref(id)
                    .expect("foreign or stale DatumRef");
                &mut self
                    .datums
                    .get_mut(datum_id)
                    .expect("validated datum disappeared")
                    .datum
            }
            DatumRef::Void => &mut self.void_datum,
        }
    }
}

impl ScriptInstanceAllocatorTrait for DatumAllocator {
    fn alloc_script_instance(&mut self, script_instance: ScriptInstance) -> ScriptInstanceRef {
        assert!(self.owner.is_arena_live(), "allocator owner is not live");
        let id = script_instance.instance_id;
        assert!(
            !self.script_instances.contains(id as usize),
            "duplicate live script instance id {}",
            id
        );
        self.script_instance_counter += 1;
        self.script_instances.insert_at(
            id as usize,
            ScriptInstanceRefEntry {
                id,
                ref_count: UnsafeCell::new(0),
                script_instance,
            },
        );
        let ref_count_ptr = self
            .script_instances
            .get(id as usize)
            .unwrap()
            .ref_count
            .get();
        ScriptInstanceRef::from_id(id, ref_count_ptr, self.owner.clone())
    }

    fn get_script_instance(&self, instance_ref: &ScriptInstanceRef) -> &ScriptInstance {
        self.get_script_instance_opt(instance_ref)
            .expect("foreign or stale ScriptInstanceRef")
    }

    fn get_script_instance_opt(
        &self,
        instance_ref: &ScriptInstanceRef,
    ) -> Option<&ScriptInstance> {
        self.valid_script_ref(instance_ref)
            .and_then(|id| self.script_instances.get(id as usize))
            .map(|entry| &entry.script_instance)
    }

    fn get_script_instance_mut(
        &mut self,
        instance_ref: &ScriptInstanceRef,
    ) -> &mut ScriptInstance {
        let id = self
            .valid_script_ref(instance_ref)
            .expect("foreign or stale ScriptInstanceRef");
        &mut self.script_instances.get_mut(id as usize).unwrap().script_instance
    }

}

impl ResetableAllocator for DatumAllocator {
    fn reset(
        &mut self,
        bitmap_manager: &mut crate::player::bitmap::manager::BitmapManager,
    ) -> OwnerToken {
        let old_owner = self.owner.clone();
        let next_owner_key = old_owner
            .key()
            .checked_next_generation()
            .expect("owner generation exhausted before allocator reset");
        let mut bitmap_guard = bitmap_manager
            .prepare_reset()
            .expect("bitmap generation exhausted before allocator reset");
        old_owner.begin_reset();
        let mut reset_guard = ResetGuard {
            owner: old_owner.clone(),
            armed: true,
        };

        // Entries whose final external handle was dropped before reset are
        // reclaimed while the old arena is still addressable. Drops caused by
        // clearing arena entries below are stale-safe and skip raw pointers.
        self.drain_reclaims(bitmap_manager);
        self.release_remaining_bitmap_refs(bitmap_manager);

        // Invalidate before the arena fields begin dropping. This is the UAF
        // boundary for every external handle carrying the old token.
        old_owner.mark_arena_dead();

        #[cfg(test)]
        if self.reset_fault {
            self.reset_fault = false;
            panic!("injected allocator reset unwind");
        }

        // Remove entries individually to ensure proper Drop cleanup.
        // Datum Drop impls may reference other datums, so reverse order
        // helps ensure dependents are dropped before their dependencies.
        debug!("Removing all datums");
        self.datums.clear_individually_reverse();

        debug!("Removing all script instances");
        self.script_instances.clear_individually();

        self.script_instance_counter = 1;

        // Re-create pools after clearing
        self.symbol_pool.clear();
        bitmap_manager
            .rotate_handles()
            .expect("bitmap generation must be available after reset precheck");
        self.owner = OwnerToken::new(next_owner_key);
        self.init_int_pool();
        bitmap_guard.commit();
        reset_guard.armed = false;
        self.owner.clone()
    }

}

struct ResetGuard {
    owner: OwnerToken,
    armed: bool,
}

impl Drop for ResetGuard {
    fn drop(&mut self) {
        if self.armed {
            self.owner.mark_arena_dead();
        }
    }
}

impl Drop for DatumAllocator {
    fn drop(&mut self) {
        // Drop runs before Rust drops the arena fields. Marking the token first
        // makes every external handle's Drop path skip its raw refcount pointer.
        self.owner.mark_arena_dead();
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    use crate::director::lingo::datum::DatumType;
    use std::collections::VecDeque;

    fn local_test_symbol() -> Symbol {
        // This value is used only as an opaque hash key. It deliberately does
        // not consult the process-global symbol interner.
        Symbol::empty()
    }

    fn allocator() -> DatumAllocator {
        DatumAllocator::new(OwnerKey::transitional())
    }

    #[test]
    fn foreign_handles_are_rejected_before_arena_access() {
        let mut first = allocator();
        let mut second = allocator();
        let mut first_bitmaps = crate::player::bitmap::manager::BitmapManager::new();
        let mut second_bitmaps = crate::player::bitmap::manager::BitmapManager::new();
        let reference = first
            .alloc_datum(Datum::String("first".into()), &mut first_bitmaps)
            .unwrap();

        assert!(second.try_get_datum(&reference).is_none());
        assert!(second.try_get_datum_mut(&reference).is_none());
        second.drain_reclaims(&mut second_bitmaps);
        drop(reference);
    }

    #[test]
    fn typed_replacement_requires_exact_live_non_pooled_target() {
        let mut alloc = allocator();
        let mut bitmaps = crate::player::bitmap::manager::BitmapManager::new();
        let vector = alloc
            .alloc_datum(Datum::Vector([1.0, 2.0, 3.0]), &mut bitmaps)
            .unwrap();
        let transform = alloc
            .alloc_datum(Datum::transform3d([0.0; 16]), &mut bitmaps)
            .unwrap();

        alloc
            .replace_vector(&vector, [4.0, 5.0, 6.0])
            .unwrap();
        assert!(matches!(
            alloc.try_get_datum(&vector),
            Some(Datum::Vector(values)) if *values == [4.0, 5.0, 6.0]
        ));
        alloc
            .replace_transform3d(&transform, [1.0; 16])
            .unwrap();
        assert!(matches!(
            alloc.try_get_datum(&transform),
            Some(Datum::Transform3d(values)) if **values == [1.0; 16]
        ));

        assert!(alloc.replace_vector(&transform, [7.0; 3]).is_err());
        assert!(matches!(
            alloc.try_get_datum(&transform),
            Some(Datum::Transform3d(values)) if **values == [1.0; 16]
        ));
        assert!(alloc.replace_transform3d(&vector, [2.0; 16]).is_err());
        assert!(matches!(
            alloc.try_get_datum(&vector),
            Some(Datum::Vector(values)) if *values == [4.0, 5.0, 6.0]
        ));

        let symbol = local_test_symbol();
        let pooled_symbol = alloc.alloc_symbol(symbol.clone());
        let same_pooled_symbol = alloc.alloc_symbol(symbol);
        assert_eq!(same_pooled_symbol, pooled_symbol);
        assert!(matches!(
            alloc.try_get_datum(&same_pooled_symbol),
            Some(Datum::Symbol(value)) if *value == local_test_symbol()
        ));
        assert!(alloc.replace_vector(&pooled_symbol, [8.0; 3]).is_err());
        assert!(alloc
            .replace_transform3d(&same_pooled_symbol, [9.0; 16])
            .is_err());
        assert!(matches!(
            alloc.try_get_datum(&pooled_symbol),
            Some(Datum::Symbol(_))
        ));
        let post_rejection_symbol = alloc.alloc_symbol(local_test_symbol());
        assert_eq!(post_rejection_symbol, pooled_symbol);
        assert!(matches!(
            alloc.try_get_datum(&post_rejection_symbol),
            Some(Datum::Symbol(value)) if *value == local_test_symbol()
        ));
    }

    #[test]
    fn typed_replacement_rejects_foreign_stale_and_bitmap_targets() {
        let mut alloc = allocator();
        let mut foreign = allocator();
        let mut bitmaps = crate::player::bitmap::manager::BitmapManager::new();
        let mut foreign_bitmaps = crate::player::bitmap::manager::BitmapManager::new();
        let local_vector = alloc
            .alloc_datum(Datum::Vector([7.0, 8.0, 9.0]), &mut bitmaps)
            .unwrap();
        let local_transform = alloc
            .alloc_datum(Datum::transform3d([10.0; 16]), &mut bitmaps)
            .unwrap();
        let foreign_vector = foreign
            .alloc_datum(Datum::Vector([1.0, 2.0, 3.0]), &mut foreign_bitmaps)
            .unwrap();
        let foreign_transform = foreign
            .alloc_datum(Datum::transform3d([11.0; 16]), &mut foreign_bitmaps)
            .unwrap();
        assert_eq!(foreign_vector.unwrap(), local_vector.unwrap());
        assert_eq!(foreign_transform.unwrap(), local_transform.unwrap());
        assert!(alloc
            .replace_vector(&foreign_vector, [4.0, 5.0, 6.0])
            .is_err());
        assert!(alloc
            .replace_transform3d(&foreign_transform, [12.0; 16])
            .is_err());
        assert!(matches!(
            alloc.try_get_datum(&local_vector),
            Some(Datum::Vector(values)) if *values == [7.0, 8.0, 9.0]
        ));
        assert!(matches!(
            alloc.try_get_datum(&local_transform),
            Some(Datum::Transform3d(values)) if **values == [10.0; 16]
        ));
        assert!(matches!(
            foreign.try_get_datum(&foreign_vector),
            Some(Datum::Vector(values)) if *values == [1.0, 2.0, 3.0]
        ));

        let stale_vector = local_vector.clone();
        let stale_transform = local_transform.clone();
        alloc.reset(&mut bitmaps);
        let fresh_vector = alloc
            .alloc_datum(Datum::Vector([13.0, 14.0, 15.0]), &mut bitmaps)
            .unwrap();
        let fresh_transform = alloc
            .alloc_datum(Datum::transform3d([16.0; 16]), &mut bitmaps)
            .unwrap();
        assert_eq!(stale_vector.unwrap(), fresh_vector.unwrap());
        assert_eq!(stale_transform.unwrap(), fresh_transform.unwrap());
        assert!(alloc
            .replace_vector(&stale_vector, [17.0; 3])
            .is_err());
        assert!(alloc
            .replace_transform3d(&stale_transform, [18.0; 16])
            .is_err());
        assert!(matches!(
            alloc.try_get_datum(&fresh_vector),
            Some(Datum::Vector(values)) if *values == [13.0, 14.0, 15.0]
        ));
        assert!(matches!(
            alloc.try_get_datum(&fresh_transform),
            Some(Datum::Transform3d(values)) if **values == [16.0; 16]
        ));

        let bitmap = crate::player::bitmap::bitmap::Bitmap::new(
            1,
            1,
            32,
            32,
            8,
            crate::player::bitmap::bitmap::PaletteRef::Default,
        );
        let bitmap_id = bitmaps.add_ephemeral_bitmap(bitmap);
        let bitmap_handle = bitmaps.local_handle(bitmap_id).unwrap();
        let bitmap_ref = alloc
            .alloc_datum(Datum::BitmapRef(bitmap_handle.clone()), &mut bitmaps)
            .unwrap();
        assert_eq!(
            bitmaps
                .ephemeral_refcount_for_test(&bitmap_handle)
                .unwrap(),
            1
        );
        assert!(alloc.replace_vector(&bitmap_ref, [13.0; 3]).is_err());
        assert!(alloc.replace_transform3d(&bitmap_ref, [14.0; 16]).is_err());
        assert!(matches!(
            alloc.try_get_datum(&bitmap_ref),
            Some(Datum::BitmapRef(handle)) if handle == &bitmap_handle
        ));
        assert_eq!(
            bitmaps
                .ephemeral_refcount_for_test(&bitmap_handle)
                .unwrap(),
            1
        );
        assert!(bitmaps.get_bitmap_handle(&bitmap_handle).is_some());
    }

    #[test]
    fn foreign_and_stale_bitmap_allocation_reject_without_mutation() {
        let mut alloc = allocator();
        let mut bitmaps = crate::player::bitmap::manager::BitmapManager::new();
        let mut foreign_bitmaps = crate::player::bitmap::manager::BitmapManager::new();
        let bitmap = crate::player::bitmap::bitmap::Bitmap::new(
            1,
            1,
            32,
            32,
            8,
            crate::player::bitmap::bitmap::PaletteRef::Default,
        );
        let local_id = bitmaps.add_ephemeral_bitmap(bitmap.clone());
        let local_handle = bitmaps.local_handle(local_id).unwrap();
        let foreign_id = foreign_bitmaps.add_ephemeral_bitmap(bitmap);
        let foreign_handle = foreign_bitmaps.local_handle(foreign_id).unwrap();
        let before_count = alloc.datum_count();

        let foreign_error = alloc
            .alloc_datum(Datum::BitmapRef(foreign_handle), &mut bitmaps)
            .unwrap_err();
        assert_eq!(foreign_error.code, ScriptErrorCode::InvalidReference);
        assert_eq!(alloc.datum_count(), before_count);
        assert_eq!(bitmaps.ephemeral_refcount_for_test(&local_handle).unwrap(), 0);

        bitmaps.rotate_handles().unwrap();
        let stale_error = alloc
            .alloc_datum(Datum::BitmapRef(local_handle), &mut bitmaps)
            .unwrap_err();
        assert_eq!(stale_error.code, ScriptErrorCode::InvalidReference);
        assert_eq!(alloc.datum_count(), before_count);
        assert_eq!(bitmaps.get_bitmap(local_id).is_some(), true);
    }

    #[test]
    fn bitmap_refcount_overflow_rolls_back_without_arena_entry() {
        let mut alloc = allocator();
        let mut bitmaps = crate::player::bitmap::manager::BitmapManager::new();
        let bitmap = crate::player::bitmap::bitmap::Bitmap::new(
            1,
            1,
            32,
            32,
            8,
            crate::player::bitmap::bitmap::PaletteRef::Default,
        );
        let bitmap_id = bitmaps.add_ephemeral_bitmap(bitmap);
        let handle = bitmaps.local_handle(bitmap_id).unwrap();
        bitmaps
            .set_ephemeral_refcount_for_test(&handle, u32::MAX)
            .unwrap();
        let before_count = alloc.datum_count();

        let overflow_error = alloc
            .alloc_datum(Datum::BitmapRef(handle.clone()), &mut bitmaps)
            .unwrap_err();
        assert_eq!(overflow_error.code, ScriptErrorCode::Generic);
        assert_eq!(alloc.datum_count(), before_count);
        assert_eq!(
            bitmaps.ephemeral_refcount_for_test(&handle).unwrap(),
            u32::MAX
        );
    }

    #[test]
    #[should_panic(expected = "arena ids are one-based")]
    fn arena_rejects_zero_based_insert_ids() {
        let mut arena = Arena::<u8>::new();
        arena.insert_at(0, 1);
    }

    #[test]
    fn deferred_reclaim_reaches_children() {
        let mut alloc = allocator();
        let mut bitmaps = crate::player::bitmap::manager::BitmapManager::new();
        let child = alloc
            .alloc_datum(Datum::String("child".into()), &mut bitmaps)
            .unwrap();
        let child_id = child.unwrap();
        let list = alloc
            .alloc_datum(
                Datum::List(
                    DatumType::List,
                    VecDeque::from([child.clone()]),
                    false,
                ),
                &mut bitmaps,
            )
            .unwrap();
        let list_id = list.unwrap();
        drop(child);
        drop(list);

        alloc.drain_reclaims(&mut bitmaps);

        assert!(!alloc.contains_datum(child_id));
        assert!(!alloc.contains_datum(list_id));
    }

    #[test]
    fn stale_clone_is_safe_after_reset_and_id_reuse() {
        let mut alloc = allocator();
        let mut bitmaps = crate::player::bitmap::manager::BitmapManager::new();
        let old = alloc
            .alloc_datum(Datum::String("old".into()), &mut bitmaps)
            .unwrap();
        let stale = old.clone();
        let old_owner = alloc.owner_token();

        let new_owner = alloc.reset(&mut bitmaps);
        assert!(!old_owner.is_arena_live());
        assert!(!alloc.owner_token().same_identity(&old_owner));
        assert!(alloc.try_get_datum(&stale).is_none());

        let reused = alloc
            .alloc_datum(Datum::String("new".into()), &mut bitmaps)
            .unwrap();
        assert_eq!(reused.unwrap(), old.unwrap());
        let stale_after_reset = stale.clone();
        assert!(alloc.try_get_datum(&reused).is_some());
        drop(stale_after_reset);
        assert!(matches!(alloc.try_get_datum(&reused), Some(Datum::String(value)) if value == "new"));
        drop(stale);
        drop(old);
        drop(reused);
        assert!(new_owner.same_identity(&alloc.owner_token()));
    }

    #[test]
    fn bitmap_effect_is_reclaimed_by_owner_queue() {
        let mut alloc = allocator();
        let mut bitmaps = crate::player::bitmap::manager::BitmapManager::new();
        let bitmap = crate::player::bitmap::bitmap::Bitmap::new(
            1,
            1,
            32,
            32,
            8,
            crate::player::bitmap::bitmap::PaletteRef::Default,
        );
        let bitmap_id = bitmaps.add_ephemeral_bitmap(bitmap);
        let bitmap_handle = bitmaps.local_handle(bitmap_id).unwrap();
        let reference = alloc
            .alloc_datum(Datum::BitmapRef(bitmap_handle), &mut bitmaps)
            .unwrap();
        assert!(bitmaps.get_bitmap(bitmap_id).is_some());
        drop(reference);
        alloc.drain_reclaims(&mut bitmaps);
        assert!(bitmaps.get_bitmap(bitmap_id).is_none());
    }

    #[test]
    fn nested_list_and_proplist_reclaim_bitmap_once() {
        let mut alloc = allocator();
        let mut bitmaps = crate::player::bitmap::manager::BitmapManager::new();
        let bitmap = crate::player::bitmap::bitmap::Bitmap::new(
            1,
            1,
            32,
            32,
            8,
            crate::player::bitmap::bitmap::PaletteRef::Default,
        );
        let bitmap_id = bitmaps.add_ephemeral_bitmap(bitmap);
        let handle = bitmaps.local_handle(bitmap_id).unwrap();
        let bitmap_ref = alloc
            .alloc_datum(Datum::BitmapRef(handle.clone()), &mut bitmaps)
            .unwrap();
        let nested_list = alloc
            .alloc_datum(
                Datum::List(DatumType::List, VecDeque::from([bitmap_ref.clone()]), false),
                &mut bitmaps,
            )
            .unwrap();
        let nested_props = alloc
            .alloc_datum(
                Datum::PropList(
                    VecDeque::from([(bitmap_ref.clone(), nested_list.clone())]),
                    false,
                ),
                &mut bitmaps,
            )
            .unwrap();
        assert_eq!(bitmaps.ephemeral_refcount_for_test(&handle).unwrap(), 1);

        drop(bitmap_ref);
        drop(nested_list);
        drop(nested_props);
        alloc.drain_reclaims(&mut bitmaps);

        assert!(bitmaps.get_bitmap(bitmap_id).is_none());
        assert!(matches!(
            bitmaps.ephemeral_refcount_for_test(&handle),
            Err(crate::player::bitmap::manager::BitmapHandleError::MissingBitmap)
        ));
    }

    #[test]
    fn reset_releases_remaining_ephemeral_bitmaps_before_epoch_change() {
        let mut alloc = allocator();
        let mut bitmaps = crate::player::bitmap::manager::BitmapManager::new();
        let bitmap = crate::player::bitmap::bitmap::Bitmap::new(
            1,
            1,
            32,
            32,
            8,
            crate::player::bitmap::bitmap::PaletteRef::Default,
        );
        let bitmap_id = bitmaps.add_ephemeral_bitmap(bitmap);
        let bitmap_handle = bitmaps.local_handle(bitmap_id).unwrap();
        let reference = alloc
            .alloc_datum(Datum::BitmapRef(bitmap_handle.clone()), &mut bitmaps)
            .unwrap();
        let _new_owner = alloc.reset(&mut bitmaps);
        assert!(bitmaps.get_bitmap_handle(&bitmap_handle).is_none());
        assert!(bitmaps.get_bitmap(bitmap_id).is_none());
        drop(reference);
    }

    #[test]
    fn reset_preserves_anchored_pixels_and_invalidates_old_bitmap_handle() {
        let mut alloc = allocator();
        let mut bitmaps = crate::player::bitmap::manager::BitmapManager::new();
        let mut anchored = crate::player::bitmap::bitmap::Bitmap::new(
            1,
            1,
            32,
            32,
            8,
            crate::player::bitmap::bitmap::PaletteRef::Default,
        );
        anchored.data[0] = 91;
        let bitmap_id = bitmaps.add_bitmap(anchored);
        let old_handle = bitmaps.local_handle(bitmap_id).unwrap();
        let retained = alloc
            .alloc_datum(Datum::BitmapRef(old_handle.clone()), &mut bitmaps)
            .unwrap();

        alloc.reset(&mut bitmaps);

        assert!(bitmaps.get_bitmap_handle(&old_handle).is_none());
        let fresh = bitmaps.local_handle(bitmap_id).unwrap();
        assert_eq!(bitmaps.get_bitmap_handle(&fresh).unwrap().data[0], 91);
        drop(retained);
    }

    #[test]
    fn script_instance_refs_reclaim_without_active_player() {
        let mut alloc = allocator();
        let mut bitmaps = crate::player::bitmap::manager::BitmapManager::new();
        let instance = ScriptInstance {
            instance_id: 42,
            script: crate::player::cast_lib::CastMemberRef {
                cast_lib: 1,
                cast_member: 1,
            },
            ancestor: None,
            properties: FxHashMap::default(),
            begin_sprite_called: false,
        };
        let reference = alloc.alloc_script_instance(instance);
        let retained = reference.clone();
        assert!(alloc.get_script_instance_opt(&reference).is_some());
        drop(reference);
        drop(retained);
        alloc.drain_reclaims(&mut bitmaps);
        assert!(alloc.get_script_instance_ref(42).is_none());
    }

    #[test]
    fn pooled_handles_are_owner_validated_across_reset() {
        let mut alloc = allocator();
        let mut bitmaps = crate::player::bitmap::manager::BitmapManager::new();
        let pooled = alloc.alloc_int(0);
        let symbol = alloc.alloc_symbol(local_test_symbol());
        let stale_pooled = pooled.clone();
        let stale_symbol = symbol.clone();
        assert!(alloc.try_get_datum(&pooled).is_some());
        assert!(alloc.try_get_datum(&symbol).is_some());

        let old_owner = alloc.owner_token();
        let _new_owner = alloc.reset(&mut bitmaps);
        assert!(!old_owner.is_arena_live());
        assert!(alloc.try_get_datum(&stale_pooled).is_none());
        assert!(alloc.try_get_datum(&stale_symbol).is_none());
        drop(stale_pooled.clone());
        drop(stale_symbol.clone());
        let fresh_int = alloc.alloc_int(0);
        let fresh_symbol = alloc.alloc_symbol(local_test_symbol());
        assert!(alloc.try_get_datum(&fresh_int).is_some());
        assert!(alloc.try_get_datum(&fresh_symbol).is_some());
    }

    #[test]
    fn allocators_are_isolated_when_created_inside_threads() {
        use std::sync::{Arc, Barrier};

        let barrier = Arc::new(Barrier::new(2));
        let first_barrier = barrier.clone();
        let first = std::thread::spawn(move || {
            let mut alloc = allocator();
            let mut bitmaps = crate::player::bitmap::manager::BitmapManager::new();
            first_barrier.wait();
            let reference = alloc
                .alloc_datum(Datum::String("first".into()), &mut bitmaps)
                .unwrap();
            let value = match alloc.try_get_datum(&reference) {
                Some(Datum::String(value)) => value.clone(),
                _ => String::new(),
            };
            drop(reference);
            alloc.drain_reclaims(&mut bitmaps);
            value
        });
        let second_barrier = barrier;
        let second = std::thread::spawn(move || {
            let mut alloc = allocator();
            let mut bitmaps = crate::player::bitmap::manager::BitmapManager::new();
            second_barrier.wait();
            let reference = alloc
                .alloc_datum(Datum::String("second".into()), &mut bitmaps)
                .unwrap();
            let value = match alloc.try_get_datum(&reference) {
                Some(Datum::String(value)) => value.clone(),
                _ => String::new(),
            };
            drop(reference);
            alloc.drain_reclaims(&mut bitmaps);
            value
        });

        assert_eq!(first.join().unwrap(), "first");
        assert_eq!(second.join().unwrap(), "second");
    }

    #[test]
    fn reset_unwind_invalidates_handles_and_rejects_allocations_until_recovery() {
        use std::panic::{catch_unwind, AssertUnwindSafe};

        let mut alloc = allocator();
        let mut bitmaps = crate::player::bitmap::manager::BitmapManager::new();
        let retained = alloc
            .alloc_datum(Datum::String("before-unwind".into()), &mut bitmaps)
            .unwrap();
        let bitmap = crate::player::bitmap::bitmap::Bitmap::new(
            1,
            1,
            32,
            32,
            8,
            crate::player::bitmap::bitmap::PaletteRef::Default,
        );
        let bitmap_id = bitmaps.add_bitmap(bitmap);
        let bitmap_handle = bitmaps.local_handle(bitmap_id).unwrap();
        let old_owner = alloc.owner_token();
        alloc.inject_reset_unwind();

        let result = catch_unwind(AssertUnwindSafe(|| alloc.reset(&mut bitmaps)));
        assert!(result.is_err());
        assert!(!old_owner.is_arena_live());
        assert!(bitmaps.get_bitmap_handle(&bitmap_handle).is_none());
        assert!(alloc
            .alloc_datum(Datum::String("rejected".into()), &mut bitmaps)
            .is_err());
        assert!(matches!(alloc.alloc_int(1), DatumRef::Void));
        assert!(matches!(alloc.alloc_symbol(local_test_symbol()), DatumRef::Void));
        let script = ScriptInstance {
            instance_id: 7,
            script: crate::player::cast_lib::CastMemberRef { cast_lib: 1, cast_member: 1 },
            ancestor: None,
            properties: FxHashMap::default(),
            begin_sprite_called: false,
        };
        assert!(catch_unwind(AssertUnwindSafe(|| alloc.alloc_script_instance(script))).is_err());

        let mut recovered = allocator();
        let mut recovered_bitmaps = crate::player::bitmap::manager::BitmapManager::new();
        assert!(recovered
            .alloc_datum(Datum::String("recovered".into()), &mut recovered_bitmaps)
            .is_ok());
        drop(retained);
    }

    #[test]
    fn reset_owner_generation_exhaustion_fails_before_allocator_mutation() {
        use std::panic::{catch_unwind, AssertUnwindSafe};

        let mut alloc = DatumAllocator::new(OwnerKey {
            session: 47,
            player: 1,
            generation: u64::MAX,
        });
        let mut bitmaps = crate::player::bitmap::manager::BitmapManager::new();
        let owner = alloc.owner_token();
        let datum = alloc
            .alloc_datum(Datum::String("retained".to_owned()), &mut bitmaps)
            .unwrap();
        let datum_count = alloc.datums.len();
        let result = catch_unwind(AssertUnwindSafe(|| alloc.reset(&mut bitmaps)));
        assert!(result.is_err());
        assert!(owner.is_arena_live());
        assert!(alloc.owner_token().same_identity(&owner));
        assert_eq!(alloc.datums.len(), datum_count);
        assert!(alloc.try_get_datum(&datum).is_some());
    }

    #[test]
    fn allocator_drop_invalidates_external_handles_before_arena_drop() {
        let mut alloc = allocator();
        let mut bitmaps = crate::player::bitmap::manager::BitmapManager::new();
        let reference = alloc
            .alloc_datum(Datum::String("drop".into()), &mut bitmaps)
            .unwrap();
        let owner = alloc.owner_token();
        drop(alloc);
        assert!(!owner.is_arena_live());
        let stale_clone = reference.clone();
        drop(stale_clone);
        drop(reference);
    }

    #[test]
    fn string_chunk_reclaims_unshared_source_but_preserves_retained_child() {
        let mut alloc = allocator();
        let mut bitmaps = crate::player::bitmap::manager::BitmapManager::new();
        let chunk_expr = crate::director::lingo::datum::StringChunkExpr {
            chunk_type: crate::director::lingo::datum::StringChunkType::Char,
            start: 1,
            end: 1,
            item_delimiter: ',',
        };

        let unshared_child = alloc
            .alloc_datum(Datum::String("unshared".to_owned()), &mut bitmaps)
            .unwrap();
        let unshared_child_id = unshared_child.unwrap();
        let unshared_chunk = alloc
            .alloc_datum(
                Datum::StringChunk(
                    crate::director::lingo::datum::StringChunkSource::Datum(
                        unshared_child.clone(),
                    ),
                    chunk_expr.clone(),
                    "u".to_owned(),
                ),
                &mut bitmaps,
            )
            .unwrap();
        let unshared_chunk_id = unshared_chunk.unwrap();
        drop(unshared_child);
        drop(unshared_chunk);
        alloc.drain_reclaims(&mut bitmaps);
        assert!(!alloc.contains_datum(unshared_chunk_id));
        assert!(!alloc.contains_datum(unshared_child_id));

        let retained_child = alloc
            .alloc_datum(Datum::String("retained".to_owned()), &mut bitmaps)
            .unwrap();
        let retained_child_id = retained_child.unwrap();
        let retained_child_clone = retained_child.clone();
        let retained_chunk = alloc
            .alloc_datum(
                Datum::StringChunk(
                    crate::director::lingo::datum::StringChunkSource::Datum(
                        retained_child.clone(),
                    ),
                    chunk_expr,
                    "r".to_owned(),
                ),
                &mut bitmaps,
            )
            .unwrap();
        let retained_chunk_id = retained_chunk.unwrap();
        drop(retained_child);
        drop(retained_chunk);
        alloc.drain_reclaims(&mut bitmaps);
        assert!(!alloc.contains_datum(retained_chunk_id));
        assert!(alloc.contains_datum(retained_child_id));

        drop(retained_child_clone);
        alloc.drain_reclaims(&mut bitmaps);
        assert!(!alloc.contains_datum(retained_child_id));
    }

    #[test]
    fn timeout_instance_reclaims_owned_children_but_preserves_retained_script_datum() {
        let mut alloc = allocator();
        let mut bitmaps = crate::player::bitmap::manager::BitmapManager::new();
        let callback = alloc
            .alloc_datum(Datum::String("callback".to_owned()), &mut bitmaps)
            .unwrap();
        let callback_id = callback.unwrap();
        let target = alloc
            .alloc_datum(Datum::String("target".to_owned()), &mut bitmaps)
            .unwrap();
        let target_id = target.unwrap();
        let script_instance = alloc.alloc_script_instance(ScriptInstance {
            instance_id: 42,
            script: crate::player::cast_lib::CastMemberRef {
                cast_lib: 1,
                cast_member: 1,
            },
            ancestor: None,
            properties: FxHashMap::default(),
            begin_sprite_called: false,
        });
        let script_datum = alloc
            .alloc_datum(Datum::ScriptInstanceRef(script_instance), &mut bitmaps)
            .unwrap();
        let script_datum_id = script_datum.unwrap();
        let retained_script_datum = script_datum.clone();
        let timeout = alloc
            .alloc_datum(
                Datum::timeout_instance(
                    "owned-timeout".to_owned(),
                    1,
                    callback.clone(),
                    target.clone(),
                    Some(script_datum.clone()),
                ),
                &mut bitmaps,
            )
            .unwrap();
        let timeout_id = timeout.unwrap();
        drop(callback);
        drop(target);
        drop(script_datum);
        drop(timeout);

        alloc.drain_reclaims(&mut bitmaps);
        assert!(!alloc.contains_datum(callback_id));
        assert!(!alloc.contains_datum(target_id));
        assert!(!alloc.contains_datum(timeout_id));
        assert!(alloc.contains_datum(script_datum_id));
        assert_eq!(alloc.script_instance_count(), 1);

        drop(retained_script_datum);
        alloc.drain_reclaims(&mut bitmaps);
        assert!(!alloc.contains_datum(script_datum_id));
        assert_eq!(alloc.script_instance_count(), 0);
    }

    #[test]
    fn stale_chunk_timeout_drops_cannot_reclaim_replacements_or_neighbor_owner() {
        let mut alloc = allocator();
        let mut bitmaps = crate::player::bitmap::manager::BitmapManager::new();
        let chunk_expr = crate::director::lingo::datum::StringChunkExpr {
            chunk_type: crate::director::lingo::datum::StringChunkType::Char,
            start: 1,
            end: 1,
            item_delimiter: ',',
        };
        let old_child = alloc
            .alloc_datum(Datum::String("old-child".to_owned()), &mut bitmaps)
            .unwrap();
        let old_child_id = old_child.unwrap();
        let old_chunk = alloc
            .alloc_datum(
                Datum::StringChunk(
                    crate::director::lingo::datum::StringChunkSource::Datum(
                        old_child.clone(),
                    ),
                    chunk_expr.clone(),
                    "o".to_owned(),
                ),
                &mut bitmaps,
            )
            .unwrap();
        let old_chunk_id = old_chunk.unwrap();
        let old_timeout = alloc
            .alloc_datum(
                Datum::timeout_instance(
                    "old-timeout".to_owned(),
                    1,
                    old_chunk.clone(),
                    DatumRef::Void,
                    None,
                ),
                &mut bitmaps,
            )
            .unwrap();
        let old_timeout_id = old_timeout.unwrap();
        let stale_child = old_child.clone();
        let stale_chunk = old_chunk.clone();
        let stale_timeout = old_timeout.clone();
        drop(old_child);
        drop(old_chunk);
        drop(old_timeout);
        alloc.reset(&mut bitmaps);

        let fresh_child = alloc
            .alloc_datum(Datum::String("fresh-child".to_owned()), &mut bitmaps)
            .unwrap();
        let fresh_chunk = alloc
            .alloc_datum(
                Datum::StringChunk(
                    crate::director::lingo::datum::StringChunkSource::Datum(
                        fresh_child.clone(),
                    ),
                    chunk_expr.clone(),
                    "f".to_owned(),
                ),
                &mut bitmaps,
            )
            .unwrap();
        let fresh_timeout = alloc
            .alloc_datum(
                Datum::timeout_instance(
                    "fresh-timeout".to_owned(),
                    1,
                    fresh_chunk.clone(),
                    DatumRef::Void,
                    None,
                ),
                &mut bitmaps,
            )
            .unwrap();
        assert_eq!(fresh_child.unwrap(), old_child_id);
        assert_eq!(fresh_chunk.unwrap(), old_chunk_id);
        assert_eq!(fresh_timeout.unwrap(), old_timeout_id);
        drop(stale_child);
        drop(stale_chunk);
        drop(stale_timeout);
        alloc.drain_reclaims(&mut bitmaps);
        assert!(matches!(
            alloc.try_get_datum(&fresh_child),
            Some(Datum::String(value)) if value == "fresh-child"
        ));
        assert!(alloc.try_get_datum(&fresh_chunk).is_some());
        assert!(alloc.try_get_datum(&fresh_timeout).is_some());

        let mut left = allocator();
        let mut right = allocator();
        let mut left_bitmaps = crate::player::bitmap::manager::BitmapManager::new();
        let mut right_bitmaps = crate::player::bitmap::manager::BitmapManager::new();
        let left_child = left
            .alloc_datum(Datum::String("left".to_owned()), &mut left_bitmaps)
            .unwrap();
        let left_chunk = left
            .alloc_datum(
                Datum::StringChunk(
                    crate::director::lingo::datum::StringChunkSource::Datum(
                        left_child.clone(),
                    ),
                    chunk_expr.clone(),
                    "l".to_owned(),
                ),
                &mut left_bitmaps,
            )
            .unwrap();
        let left_timeout = left
            .alloc_datum(
                Datum::timeout_instance(
                    "left-timeout".to_owned(),
                    1,
                    left_chunk.clone(),
                    DatumRef::Void,
                    None,
                ),
                &mut left_bitmaps,
            )
            .unwrap();
        let right_child = right
            .alloc_datum(Datum::String("right".to_owned()), &mut right_bitmaps)
            .unwrap();
        let right_chunk = right
            .alloc_datum(
                Datum::StringChunk(
                    crate::director::lingo::datum::StringChunkSource::Datum(
                        right_child.clone(),
                    ),
                    chunk_expr,
                    "r".to_owned(),
                ),
                &mut right_bitmaps,
            )
            .unwrap();
        let right_timeout = right
            .alloc_datum(
                Datum::timeout_instance(
                    "right-timeout".to_owned(),
                    1,
                    right_chunk.clone(),
                    DatumRef::Void,
                    None,
                ),
                &mut right_bitmaps,
            )
            .unwrap();
        assert_eq!(left_child.unwrap(), right_child.unwrap());
        assert_eq!(left_chunk.unwrap(), right_chunk.unwrap());
        assert_eq!(left_timeout.unwrap(), right_timeout.unwrap());
        assert_eq!(left.owner_token().key(), right.owner_token().key());
        assert!(!left.owner_token().same_identity(&right.owner_token()));

        drop(left_child);
        drop(left_chunk);
        drop(left_timeout);
        left.drain_reclaims(&mut left_bitmaps);
        assert!(right.try_get_datum(&right_child).is_some());
        assert!(right.try_get_datum(&right_chunk).is_some());
        assert!(right.try_get_datum(&right_timeout).is_some());
    }
}
