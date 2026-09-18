use std::{cell::Cell, collections::HashMap, rc::Rc};

use super::{
    bitmap::{Bitmap, BuiltInPalette, PaletteRef},
    palette_map::PaletteMap,
};
use crate::player::{
    cast_lib::CastMemberRef, handlers::datum_handlers::cast_member_ref::CastMemberRefHandlers,
};

/// A manager-local allocation identity.
///
/// This remains a plain integer for cast/render state while retained Datum
/// references migrate to [`BitmapHandle`]. It must not cross a manager
/// boundary without an explicit pixel copy.
pub type BitmapId = u32;

/// Compatibility alias for existing manager-local bitmap state.
pub const INVALID_BITMAP_ID: BitmapId = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitmapHandleError {
    InvalidHandle,
    MissingBitmap,
    RefCountUnderflow,
    RefCountOverflow,
    IdExhausted,
    GenerationExhausted,
}

struct BitmapOwner {
    generation: u64,
    live: Cell<bool>,
}

/// A retained bitmap capability bound to one manager generation.
///
/// The private fields prevent callers from manufacturing a handle for an
/// arbitrary manager-local ID. `BitmapManager::local_handle` is the only
/// rebinding point, and it can only qualify an ID that is present in that
/// manager. Manager accessors validate the owner identity before touching the
/// bitmap map or ephemeral refcount table.
#[derive(Clone)]
pub struct BitmapHandle {
    id: BitmapId,
    owner: Rc<BitmapOwner>,
}

impl std::fmt::Debug for BitmapHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BitmapHandle")
            .field("id", &self.id)
            .finish()
    }
}

impl PartialEq for BitmapHandle {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && Rc::ptr_eq(&self.owner, &other.owner)
    }
}

impl Eq for BitmapHandle {}

pub struct BitmapManager {
    bitmaps: HashMap<BitmapId, Bitmap>,
    ref_counter: BitmapId,
    owner: Rc<BitmapOwner>,
    /// Side table for ephemeral bitmaps — those produced by Lingo getters
    /// like `(the stage).image`, `image(w, h, d)`, `bitmap.duplicate()`,
    /// member `.image` accessors, etc. The value is the number of
    /// `Datum::BitmapRef` arena entries currently pointing at the bitmap;
    /// when it drops to zero the bitmap is freed.
    ///
    /// Cast-member-owned bitmaps are NOT in this map and are never freed by
    /// the refcount path — they live as long as the cast member does.
    ephemeral_refs: HashMap<BitmapId, u32>,
}

pub(crate) struct BitmapResetGuard {
    owner: Rc<BitmapOwner>,
    committed: bool,
}

fn palette_is_present(palettes: &PaletteMap, member_ref: &CastMemberRef) -> bool {
    if member_ref.cast_lib == 0 {
        return palettes
            .find_by_member(member_ref.cast_member as u32)
            .is_some();
    }

    let slot_number = CastMemberRefHandlers::get_cast_slot_number(
        member_ref.cast_lib as u32,
        member_ref.cast_member as u32,
    );
    palettes.get(slot_number as usize).is_some()
        || palettes
            .find_by_member(member_ref.cast_member as u32)
            .is_some()
        || palettes
            .find_by_cast_lib(member_ref.cast_lib as u32)
            .is_some()
}

impl BitmapResetGuard {
    pub(crate) fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for BitmapResetGuard {
    fn drop(&mut self) {
        if !self.committed {
            self.owner.live.set(false);
        }
    }
}

impl BitmapManager {
    pub fn new() -> Self {
        let owner = Rc::new(BitmapOwner {
            generation: 0,
            live: Cell::new(true),
        });
        Self {
            bitmaps: HashMap::new(),
            ref_counter: 0,
            owner,
            ephemeral_refs: HashMap::new(),
        }
    }

    fn next_id(&mut self) -> Result<BitmapId, BitmapHandleError> {
        let id = self
            .ref_counter
            .checked_add(1)
            .ok_or(BitmapHandleError::IdExhausted)?;
        debug_assert_ne!(id, INVALID_BITMAP_ID);
        self.ref_counter = id;
        Ok(id)
    }

    fn checked_handle_id(&self, handle: &BitmapHandle) -> Result<BitmapId, BitmapHandleError> {
        if !handle.owner.live.get() || !Rc::ptr_eq(&handle.owner, &self.owner) {
            return Err(BitmapHandleError::InvalidHandle);
        }
        Ok(handle.id)
    }

    pub(crate) fn prepare_reset(&self) -> Result<BitmapResetGuard, BitmapHandleError> {
        self.owner
            .generation
            .checked_add(1)
            .ok_or(BitmapHandleError::GenerationExhausted)?;
        Ok(BitmapResetGuard {
            owner: Rc::clone(&self.owner),
            committed: false,
        })
    }

    fn insert_bitmap_handle(
        &mut self,
        bitmap: Bitmap,
        ephemeral: bool,
    ) -> Result<BitmapHandle, BitmapHandleError> {
        let id = self.next_id()?;
        self.bitmaps.insert(id, bitmap);
        if ephemeral {
            self.ephemeral_refs.insert(id, 0);
        }
        Ok(BitmapHandle {
            id,
            owner: Rc::clone(&self.owner),
        })
    }

    /// Qualify a manager-local ID for the current handle generation.
    ///
    /// This is intentionally the only raw-ID-to-handle conversion. An old
    /// handle must never be reduced to an ID and rebound here by a caller.
    pub(crate) fn local_handle(&self, bitmap_id: BitmapId) -> Option<BitmapHandle> {
        if bitmap_id == INVALID_BITMAP_ID || !self.bitmaps.contains_key(&bitmap_id) {
            return None;
        }
        Some(BitmapHandle {
            id: bitmap_id,
            owner: Rc::clone(&self.owner),
        })
    }

    /// Return the local ID only after proving that `handle` belongs to this
    /// live manager generation and still names a present bitmap.
    pub fn local_id(&self, handle: &BitmapHandle) -> Option<BitmapId> {
        let id = self.checked_handle_id(handle).ok()?;
        self.bitmaps.contains_key(&id).then_some(id)
    }

    /// Add an anchored bitmap and return a retained capability for it.
    pub fn add_bitmap_handle(&mut self, bitmap: Bitmap) -> Result<BitmapHandle, BitmapHandleError> {
        self.insert_bitmap_handle(bitmap, false)
    }

    /// Add an ephemeral bitmap and return a retained capability for it.
    pub fn add_ephemeral_bitmap_handle(
        &mut self,
        bitmap: Bitmap,
    ) -> Result<BitmapHandle, BitmapHandleError> {
        self.insert_bitmap_handle(bitmap, true)
    }

    pub fn get_bitmap_handle(&self, handle: &BitmapHandle) -> Option<&Bitmap> {
        let id = self.checked_handle_id(handle).ok()?;
        self.bitmaps.get(&id)
    }

    pub fn get_bitmap_handle_mut(&mut self, handle: &BitmapHandle) -> Option<&mut Bitmap> {
        let id = self.checked_handle_id(handle).ok()?;
        let bitmap = self.bitmaps.get_mut(&id)?;
        bitmap.version = bitmap.version.wrapping_add(1);
        Some(bitmap)
    }

    pub fn replace_bitmap_handle(
        &mut self,
        handle: &BitmapHandle,
        mut bitmap: Bitmap,
    ) -> Result<(), BitmapHandleError> {
        let id = self.checked_handle_id(handle)?;
        let Some(old_bitmap) = self.bitmaps.get(&id) else {
            return Err(BitmapHandleError::MissingBitmap);
        };
        bitmap.version = old_bitmap.version.wrapping_add(1);
        self.bitmaps.insert(id, bitmap);
        Ok(())
    }

    pub fn incref_ephemeral_handle(
        &mut self,
        handle: &BitmapHandle,
    ) -> Result<(), BitmapHandleError> {
        let id = self.checked_handle_id(handle)?;
        if !self.bitmaps.contains_key(&id) {
            return Err(BitmapHandleError::MissingBitmap);
        }
        if let Some(count) = self.ephemeral_refs.get_mut(&id) {
            *count = count
                .checked_add(1)
                .ok_or(BitmapHandleError::RefCountOverflow)?;
        }
        Ok(())
    }

    pub fn decref_ephemeral_handle(
        &mut self,
        handle: &BitmapHandle,
    ) -> Result<(), BitmapHandleError> {
        let id = self.checked_handle_id(handle)?;
        if !self.bitmaps.contains_key(&id) {
            return Err(BitmapHandleError::MissingBitmap);
        }
        let should_free = if let Some(count) = self.ephemeral_refs.get_mut(&id) {
            if *count == 0 {
                return Err(BitmapHandleError::RefCountUnderflow);
            }
            *count -= 1;
            *count == 0
        } else {
            false
        };
        if should_free {
            self.ephemeral_refs.remove(&id);
            self.bitmaps.remove(&id);
        }
        Ok(())
    }

    /// Copy pixels from a source manager into this manager. Indexed pixels are
    /// resolved through the supplied movie palette map and explicit builtin
    /// fallback, so the result has no ambient palette dependency. The source
    /// handle is validated by the source manager; its numeric ID is never
    /// rebound in the destination. The copy is ephemeral because the returned
    /// handle is intended for a newly retained Datum value.
    pub fn copy_bitmap_from(
        &mut self,
        source: &BitmapManager,
        source_handle: &BitmapHandle,
        palettes: &PaletteMap,
        default_palette: BuiltInPalette,
    ) -> Result<BitmapHandle, BitmapHandleError> {
        let source_bitmap = source
            .get_bitmap_handle(source_handle)
            .ok_or(BitmapHandleError::InvalidHandle)?
            .clone();
        let palette_ref = match &source_bitmap.palette_ref {
            PaletteRef::BuiltIn(palette) => PaletteRef::BuiltIn(*palette),
            PaletteRef::Default => PaletteRef::BuiltIn(default_palette),
            PaletteRef::Member(member_ref) if palette_is_present(palettes, member_ref) => {
                PaletteRef::Member(member_ref.clone())
            }
            PaletteRef::Member(_) => PaletteRef::BuiltIn(default_palette),
        };

        let mut normalized_source = source_bitmap.clone();
        normalized_source.palette_ref = palette_ref;
        let mut bitmap = Bitmap::new(
            normalized_source.width,
            normalized_source.height,
            32,
            32,
            if normalized_source.use_alpha { 8 } else { 0 },
            PaletteRef::BuiltIn(default_palette),
        );
        bitmap.data.clear();
        bitmap
            .data
            .reserve(normalized_source.width as usize * normalized_source.height as usize * 4);
        for y in 0..normalized_source.height {
            for x in 0..normalized_source.width {
                let (red, green, blue, alpha) =
                    normalized_source.get_pixel_color_with_alpha(palettes, x, y);
                bitmap.data.extend_from_slice(&[red, green, blue, alpha]);
            }
        }
        bitmap.use_alpha = normalized_source.use_alpha;
        bitmap.matte = normalized_source.matte.clone();
        bitmap.trim_white_space = normalized_source.trim_white_space;
        bitmap.was_trimmed = normalized_source.was_trimmed;
        self.add_ephemeral_bitmap_handle(bitmap)
    }

    /// Invalidate all retained capabilities while preserving manager-local
    /// bitmap IDs and pixels for fresh local qualification.
    pub fn rotate_handles(&mut self) -> Result<(), BitmapHandleError> {
        let generation = self
            .owner
            .generation
            .checked_add(1)
            .ok_or(BitmapHandleError::GenerationExhausted)?;
        self.owner.live.set(false);
        self.owner = Rc::new(BitmapOwner {
            generation,
            live: Cell::new(true),
        });
        Ok(())
    }

    /// Drop every stored bitmap when switching movies. Cast-member-owned
    /// (anchored) bitmaps are never removed by the ephemeral refcount path, so
    /// without this they orphan here forever: loading a new movie replaces the
    /// cast list but leaks the previous movie's bitmaps (Infestation's ~363
    /// bitmaps, ~10 MB+ decoded, on every load — memory that never comes back).
    /// `ref_counter` is NOT reset so freshly-issued refs can't collide with any
    /// `Datum::BitmapRef` that a persisted global still holds.
    pub(crate) fn clear_movie_bitmaps(&mut self) {
        self.bitmaps.clear();
        self.ephemeral_refs.clear();
    }

    /// Remove one manager-local ephemeral allocation after the owning Datum
    /// arena has been swept. Anchored cast-member pixels are never removed by
    /// this path.
    pub(crate) fn remove_ephemeral_bitmap(&mut self, bitmap_id: BitmapId) {
        if self.ephemeral_refs.remove(&bitmap_id).is_some() {
            self.bitmaps.remove(&bitmap_id);
        }
    }

    #[cfg(test)]
    pub(crate) fn set_ephemeral_refcount_for_test(
        &mut self,
        handle: &BitmapHandle,
        count: u32,
    ) -> Result<(), BitmapHandleError> {
        let id = self.checked_handle_id(handle)?;
        let refs = self
            .ephemeral_refs
            .get_mut(&id)
            .ok_or(BitmapHandleError::MissingBitmap)?;
        *refs = count;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn ephemeral_refcount_for_test(
        &self,
        handle: &BitmapHandle,
    ) -> Result<u32, BitmapHandleError> {
        let id = self.checked_handle_id(handle)?;
        self.ephemeral_refs
            .get(&id)
            .copied()
            .ok_or(BitmapHandleError::MissingBitmap)
    }

    /// Register an anchored bitmap (owned by a cast member or other long-lived
    /// holder). Will not be auto-freed when DatumRefs drop. This compatibility
    /// API returns a manager-local ID; retained values must use a handle API.
    pub(crate) fn add_bitmap(&mut self, bitmap: Bitmap) -> BitmapId {
        self.add_bitmap_handle(bitmap)
            .expect("bitmap ID space exhausted")
            .id
    }

    /// Register an ephemeral bitmap. Once the last `Datum::BitmapRef(N)`
    /// arena entry is dropped, the bitmap is freed. Use for `(the stage)
    /// .image`, `image(w, h, d)`, `bitmap.duplicate()`, member `.image`
    /// snapshots — anywhere a Lingo expression produces a bitmap with no
    /// other persistent owner.
    pub(crate) fn add_ephemeral_bitmap(&mut self, bitmap: Bitmap) -> BitmapId {
        let bitmap_ref = self
            .add_ephemeral_bitmap_handle(bitmap)
            .expect("bitmap ID space exhausted")
            .id;
        // Start at 0 — the caller's `alloc_datum(Datum::BitmapRef(...))` will
        // bump it via `incref_ephemeral`. If for some reason the bitmap is
        // never wrapped in a DatumRef the entry leaks, but that's rare and
        // strictly better than the previous always-leak behaviour.
        bitmap_ref
    }

    /// Manager-local compatibility API. Retained callers must use
    /// `replace_bitmap_handle` so owner validation happens before mutation.
    pub(crate) fn replace_bitmap(&mut self, bitmap_ref: BitmapId, bitmap: Bitmap) {
        let Some(handle) = self.local_handle(bitmap_ref) else {
            return;
        };
        let _ = self.replace_bitmap_handle(&handle, bitmap);
    }

    /// Manager-local compatibility API. Retained callers must use
    /// `get_bitmap_handle` so owner validation happens before access.
    #[allow(dead_code)]
    pub(crate) fn get_bitmap(&self, bitmap_ref: BitmapId) -> Option<&Bitmap> {
        self.bitmaps.get(&bitmap_ref)
    }

    /// Manager-local compatibility API. Retained callers must use
    /// `get_bitmap_handle_mut` so owner validation happens before mutation.
    #[allow(dead_code)]
    pub(crate) fn get_bitmap_mut(&mut self, bitmap_ref: BitmapId) -> Option<&mut Bitmap> {
        // Increment version when giving mutable access, as the bitmap may be
        // modified. This ensures texture caches know to re-upload the texture.
        if let Some(bitmap) = self.bitmaps.get_mut(&bitmap_ref) {
            bitmap.version = bitmap.version.wrapping_add(1);
            Some(bitmap)
        } else {
            None
        }
    }
}

impl Drop for BitmapManager {
    fn drop(&mut self) {
        self.owner.live.set(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::bitmap::bitmap::{resolve_color_ref, Bitmap, PaletteRef};
    use crate::player::cast_member::PaletteMember;
    use crate::player::sprite::ColorRef;

    fn bitmap(color: u8) -> Bitmap {
        let mut bitmap = Bitmap::new(1, 1, 32, 32, 8, PaletteRef::Default);
        bitmap.data.fill(color);
        bitmap
    }

    #[test]
    fn handles_reject_same_id_from_another_manager_before_access() {
        let mut first = BitmapManager::new();
        let mut second = BitmapManager::new();
        let first_handle = first.add_bitmap_handle(bitmap(1)).unwrap();
        let second_handle = second.add_bitmap_handle(bitmap(2)).unwrap();

        assert_eq!(first_handle.id, second_handle.id);
        assert!(second.get_bitmap_handle(&first_handle).is_none());
        assert!(matches!(
            second.replace_bitmap_handle(&first_handle, bitmap(3)),
            Err(BitmapHandleError::InvalidHandle)
        ));
        assert!(matches!(
            second.incref_ephemeral_handle(&first_handle),
            Err(BitmapHandleError::InvalidHandle)
        ));
        assert!(matches!(
            second.decref_ephemeral_handle(&first_handle),
            Err(BitmapHandleError::InvalidHandle)
        ));
        assert_eq!(second.get_bitmap_handle(&second_handle).unwrap().data[0], 2);
    }

    #[test]
    fn copy_requires_source_capability_and_creates_independent_bitmap() {
        let mut source = BitmapManager::new();
        let mut destination = BitmapManager::new();
        let source_handle = source.add_bitmap_handle(bitmap(7)).unwrap();
        let palettes = PaletteMap::new();

        let copied = destination
            .copy_bitmap_from(
                &source,
                &source_handle,
                &palettes,
                BuiltInPalette::SystemWin,
            )
            .unwrap();
        assert_eq!(copied.id, source_handle.id);
        assert_eq!(destination.get_bitmap_handle(&copied).unwrap().data[0], 7);

        destination.get_bitmap_handle_mut(&copied).unwrap().data[0] = 9;
        assert_eq!(source.get_bitmap_handle(&source_handle).unwrap().data[0], 7);

        source.rotate_handles().unwrap();
        assert!(matches!(
            destination.copy_bitmap_from(
                &source,
                &source_handle,
                &palettes,
                BuiltInPalette::SystemWin,
            ),
            Err(BitmapHandleError::InvalidHandle)
        ));
    }

    #[test]
    fn copy_normalizes_indexed_pixels_with_source_palette_and_colliding_ids() {
        let mut source = BitmapManager::new();
        let mut destination = BitmapManager::new();
        let mut source_bitmap = Bitmap::new(
            1,
            1,
            8,
            8,
            0,
            PaletteRef::Member(crate::player::cast_lib::CastMemberRef {
                cast_lib: 1,
                cast_member: 1,
            }),
        );
        source_bitmap.data[0] = 7;
        let source_handle = source.add_bitmap_handle(source_bitmap).unwrap();
        let destination_collision = destination.add_bitmap_handle(bitmap(91)).unwrap();
        assert_eq!(source_handle.id, destination_collision.id);

        let mut palettes = PaletteMap::new();
        let mut palette = PaletteMember::new();
        palette.colors[7] = (10, 20, 30);
        palettes.insert(1, palette);
        let copied = destination
            .copy_bitmap_from(
                &source,
                &source_handle,
                &palettes,
                BuiltInPalette::SystemWin,
            )
            .unwrap();
        let copied_bitmap = destination.get_bitmap_handle(&copied).unwrap();
        assert_eq!(copied.id, 2);
        assert_eq!(copied_bitmap.bit_depth, 32);
        assert_eq!(copied_bitmap.data, vec![10, 20, 30, 255]);
        assert_eq!(
            copied_bitmap.palette_ref,
            PaletteRef::BuiltIn(BuiltInPalette::SystemWin)
        );

        source.get_bitmap_handle_mut(&source_handle).unwrap().data[0] = 2;
        assert_eq!(
            destination.get_bitmap_handle(&copied).unwrap().data,
            vec![10, 20, 30, 255]
        );
        palettes.palettes[0].member.colors[7] = (200, 210, 220);
        assert_eq!(
            destination.get_bitmap_handle(&copied).unwrap().data,
            vec![10, 20, 30, 255]
        );
    }

    #[test]
    fn copy_default_palette_uses_explicit_builtin_without_ambient_lookup() {
        let mut source = BitmapManager::new();
        let mut destination = BitmapManager::new();
        let mut source_bitmap = Bitmap::new(1, 1, 8, 8, 0, PaletteRef::Default);
        source_bitmap.data[0] = 1;
        let source_handle = source.add_bitmap_handle(source_bitmap).unwrap();
        let palettes = PaletteMap::new();

        let copied = destination
            .copy_bitmap_from(
                &source,
                &source_handle,
                &palettes,
                BuiltInPalette::GrayScale,
            )
            .unwrap();
        let copied_bitmap = destination.get_bitmap_handle(&copied).unwrap();
        let expected = resolve_color_ref(
            &palettes,
            &ColorRef::PaletteIndex(1),
            &PaletteRef::BuiltIn(BuiltInPalette::GrayScale),
            8,
        );
        assert_eq!(
            copied_bitmap.palette_ref,
            PaletteRef::BuiltIn(BuiltInPalette::GrayScale)
        );
        assert_eq!(
            copied_bitmap.data,
            vec![expected.0, expected.1, expected.2, 255]
        );
    }

    #[test]
    fn copy_missing_member_palette_uses_explicit_fallback_and_is_independent() {
        let mut source = BitmapManager::new();
        let mut destination = BitmapManager::new();
        let mut source_bitmap = Bitmap::new(
            1,
            1,
            8,
            8,
            0,
            PaletteRef::Member(crate::player::cast_lib::CastMemberRef {
                cast_lib: 9,
                cast_member: 9,
            }),
        );
        source_bitmap.data[0] = 1;
        let source_handle = source.add_bitmap_handle(source_bitmap).unwrap();
        let palettes = PaletteMap::new();
        let copied = destination
            .copy_bitmap_from(
                &source,
                &source_handle,
                &palettes,
                BuiltInPalette::GrayScale,
            )
            .unwrap();
        let expected = resolve_color_ref(
            &palettes,
            &ColorRef::PaletteIndex(1),
            &PaletteRef::BuiltIn(BuiltInPalette::GrayScale),
            8,
        );
        assert_eq!(
            destination.get_bitmap_handle(&copied).unwrap().data,
            vec![expected.0, expected.1, expected.2, 255]
        );
        source.get_bitmap_handle_mut(&source_handle).unwrap().data[0] = 7;
        assert_eq!(
            destination.get_bitmap_handle(&copied).unwrap().data,
            vec![expected.0, expected.1, expected.2, 255]
        );
    }

    #[test]
    fn bitmap_id_exhaustion_leaves_manager_state_unchanged() {
        let mut manager = BitmapManager::new();
        let current = manager.add_bitmap_handle(bitmap(3)).unwrap();
        let before_bitmap_count = manager.bitmaps.len();
        let before_ephemeral_count = manager.ephemeral_refs.len();
        manager.ref_counter = BitmapId::MAX;

        assert!(matches!(
            manager.add_ephemeral_bitmap_handle(bitmap(4)),
            Err(BitmapHandleError::IdExhausted)
        ));
        assert_eq!(manager.ref_counter, BitmapId::MAX);
        assert_eq!(manager.bitmaps.len(), before_bitmap_count);
        assert_eq!(manager.ephemeral_refs.len(), before_ephemeral_count);
        assert_eq!(manager.get_bitmap_handle(&current).unwrap().data[0], 3);
    }

    #[test]
    fn bitmap_generation_exhaustion_leaves_manager_state_unchanged() {
        let mut manager = BitmapManager::new();
        manager.owner = Rc::new(BitmapOwner {
            generation: u64::MAX,
            live: Cell::new(true),
        });
        let current = manager.add_bitmap_handle(bitmap(5)).unwrap();
        let before_bitmap_count = manager.bitmaps.len();
        let before_ephemeral_count = manager.ephemeral_refs.len();

        assert!(matches!(
            manager.prepare_reset(),
            Err(BitmapHandleError::GenerationExhausted)
        ));
        assert_eq!(manager.owner.generation, u64::MAX);
        assert!(manager.owner.live.get());
        assert!(Rc::ptr_eq(&current.owner, &manager.owner));
        assert_eq!(manager.bitmaps.len(), before_bitmap_count);
        assert_eq!(manager.ephemeral_refs.len(), before_ephemeral_count);
        assert_eq!(manager.get_bitmap_handle(&current).unwrap().data[0], 5);
    }

    #[test]
    fn rotation_and_drop_invalidate_old_handles_without_clearing_local_bitmaps() {
        let mut manager = BitmapManager::new();
        let old = manager.add_bitmap_handle(bitmap(4)).unwrap();
        let old_id = old.id;

        manager.rotate_handles().unwrap();
        assert!(manager.get_bitmap_handle(&old).is_none());
        assert!(matches!(
            manager.replace_bitmap_handle(&old, bitmap(5)),
            Err(BitmapHandleError::InvalidHandle)
        ));

        let fresh = manager.local_handle(old_id).unwrap();
        assert_eq!(manager.get_bitmap_handle(&fresh).unwrap().data[0], 4);

        let dropped = manager.add_bitmap_handle(bitmap(6)).unwrap();
        drop(manager);
        assert!(!dropped.owner.live.get());
    }

    #[test]
    fn stale_ephemeral_handle_cannot_decrement_missing_bitmap() {
        let mut manager = BitmapManager::new();
        let handle = manager.add_ephemeral_bitmap_handle(bitmap(8)).unwrap();
        manager.incref_ephemeral_handle(&handle).unwrap();
        manager.decref_ephemeral_handle(&handle).unwrap();

        assert!(manager.get_bitmap_handle(&handle).is_none());
        assert!(matches!(
            manager.decref_ephemeral_handle(&handle),
            Err(BitmapHandleError::MissingBitmap)
        ));
    }

    #[test]
    fn stale_handle_rejects_mutation_and_refcount_after_rotation() {
        let mut manager = BitmapManager::new();
        let handle = manager.add_ephemeral_bitmap_handle(bitmap(1)).unwrap();
        manager.incref_ephemeral_handle(&handle).unwrap();
        manager.rotate_handles().unwrap();

        assert!(manager.get_bitmap_handle(&handle).is_none());
        assert!(matches!(manager.get_bitmap_handle_mut(&handle), None));
        assert!(matches!(
            manager.incref_ephemeral_handle(&handle),
            Err(BitmapHandleError::InvalidHandle)
        ));
        assert!(matches!(
            manager.decref_ephemeral_handle(&handle),
            Err(BitmapHandleError::InvalidHandle)
        ));
    }

    #[test]
    fn checked_refcount_overflow_and_underflow_leave_state_unchanged() {
        let mut manager = BitmapManager::new();
        let handle = manager.add_ephemeral_bitmap_handle(bitmap(2)).unwrap();
        manager
            .set_ephemeral_refcount_for_test(&handle, u32::MAX)
            .unwrap();
        assert!(matches!(
            manager.incref_ephemeral_handle(&handle),
            Err(BitmapHandleError::RefCountOverflow)
        ));
        assert_eq!(
            manager.ephemeral_refcount_for_test(&handle).unwrap(),
            u32::MAX
        );

        manager.set_ephemeral_refcount_for_test(&handle, 0).unwrap();
        assert!(matches!(
            manager.decref_ephemeral_handle(&handle),
            Err(BitmapHandleError::RefCountUnderflow)
        ));
        assert_eq!(manager.ephemeral_refcount_for_test(&handle).unwrap(), 0);
        assert!(manager.get_bitmap_handle(&handle).is_some());
    }
}
