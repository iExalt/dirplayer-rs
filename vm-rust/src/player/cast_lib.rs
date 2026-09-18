use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc};

use fxhash::FxHashMap;
use url::Url;

use crate::{
    director::{
        cast::CastDef,
        chunks::sound::SoundChunk,
        enums::{BitmapInfo, ScriptType, SoundInfo},
        file::{read_director_file_bytes, DirectorFile},
        lingo::{datum::Datum, script::ScriptContext},
    },
    player::{
        cast_member::ScriptMember,
        symbols::{builtin::BuiltInSymbol, symbol::Symbol, symbol_table::SymbolTable},
    },
    utils::{get_base_url, get_basename_no_extension, log_i},
};

use super::{
    allocator::DatumAllocator,
    bitmap::{
        bitmap::{Bitmap, BuiltInPalette, PaletteRef},
        manager::BitmapManager,
    },
    cast_member::{
        BitmapMember, CastMember, CastMemberType, FieldMember, FlashMember, MovieMember,
        PaletteMember, SoundMember, TextMember, VectorShapeMember,
    },
    datum_ref::DatumRef,
    handlers::datum_handlers::cast_member_ref::CastMemberRefHandlers,
    host_events::HostEvent,
    net_manager::resolve_preload_url,
    ownership::{OwnerKey, OwnerToken},
    reserve_player_mut,
    script::Script,
    ScriptError,
};

pub type CastLibNumber = u32;
pub type CastMemberNumber = u32;
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastLibState {
    None,
    Loading,
    Loaded,
}

#[derive(Debug)]
pub struct CastLoadCapability;

#[derive(Clone, Debug)]
pub struct CastLoadRequest {
    owner: OwnerKey,
    cast_number: u32,
    file_name: String,
    /// Fully resolved URL/path for the external adapter.
    requested_url: String,
    /// Original cast path retained for diagnostics.
    source_path: String,
    cache_key: Box<str>,
    capability: Arc<CastLoadCapability>,
}

#[derive(Debug)]
pub struct CastLoadResult {
    capability: Arc<CastLoadCapability>,
    /// Adapter-resolved URL retained for DirectorFile base-path parsing.
    resolved_url: String,
    bytes: Result<Vec<u8>, String>,
}

/// The property setter must distinguish an empty filename from a load that
/// needs to remain owned by the session.  `Option<CastLoadRequest>` cannot
/// represent that distinction because preload reservations also use `None`
/// for an already-loading cast.
#[derive(Debug)]
pub enum PropertyLoadPreparation {
    Empty,
    Request(CastLoadRequest),
}

impl CastLoadRequest {
    /// Fully resolved URL/path that the external adapter should fetch.
    pub fn requested_url(&self) -> &str {
        &self.requested_url
    }

    /// Original cast path, retained for adapter diagnostics.
    pub fn source_path(&self) -> &str {
        &self.source_path
    }

    pub fn cast_number(&self) -> u32 {
        self.cast_number
    }

    /// Build a completion without exposing or allowing replacement of the
    /// authoritative owner, cast, cache, or capability metadata.
    pub fn complete(&self, resolved_url: String, bytes: Result<Vec<u8>, String>) -> CastLoadResult {
        CastLoadResult {
            capability: self.capability.clone(),
            resolved_url,
            bytes,
        }
    }

    pub(crate) fn owner_key(&self) -> OwnerKey {
        self.owner
    }

    pub(crate) fn cache_key(&self) -> &str {
        &self.cache_key
    }

    pub(crate) fn capability(&self) -> &Arc<CastLoadCapability> {
        &self.capability
    }
}

impl CastLoadResult {
    pub(crate) fn capability(&self) -> &Arc<CastLoadCapability> {
        &self.capability
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CastNotification {
    CastMemberNameChanged(u32),
    CastMemberListChanged(u32),
    CastNameChanged(u32),
    CastListChanged,
}

/// Owner-local notifications produced while a player is mutably borrowed.
/// These contain no JS values; the owning session extracts snapshots and
/// dispatches them only after releasing the player borrow.
#[derive(Clone, Debug)]
pub enum PlayerNotificationKind {
    ScoreChanged,
    ChannelChanged(i16),
    ChannelNameChanged(i16),
    ChannelNamesChanged,
    CastMemberChanged(CastMemberRef),
    CastMemberListChanged(u32),
    CastMemberNameChanged(u32),
    DatumSnapshot(crate::player::datum_ref::DatumRef),
    ScriptInstanceSnapshot(Option<crate::player::script_ref::ScriptInstanceRef>),
    /// An owner-local portable event.  The payload contains no arena or JS
    /// references and is dispatched only after the player borrow ends.
    Host(HostEvent),
    /// Terminal owner-local backpressure. The producer could not retain the
    /// event within its bounded mailbox; the detached drain must stop and
    /// report this failure so the owner can reset/rebind explicitly.
    HostBackpressure(usize),
}

#[derive(Clone, Debug)]
pub struct PlayerNotification {
    pub(crate) owner: OwnerToken,
    pub(crate) kind: PlayerNotificationKind,
}

#[derive(Default, Debug)]
pub struct CastNotificationOutbox {
    events: Vec<CastNotification>,
}

impl CastNotificationOutbox {
    pub(crate) fn push(&mut self, event: CastNotification) {
        self.events.push(event);
    }

    pub fn drain(&mut self) -> Vec<CastNotification> {
        std::mem::take(&mut self.events)
    }
}

pub struct CastLib {
    pub name: String,
    pub file_name: String,
    pub number: u32,
    pub is_external: bool,
    pub state: CastLibState,
    pub(crate) pending_load: Option<Arc<CastLoadCapability>>,
    pub lctx: Option<ScriptContext>,
    pub members: FxHashMap<u32, CastMember>,
    pub scripts: FxHashMap<u32, Rc<Script>>,
    /// JS-Lingo programs decoded while the cast is mutably borrowed. The
    /// owning session drains this queue after the cast borrow ends, then runs
    /// each top-level program before publishing its runtime for dispatch.
    pub(crate) pending_js_registrations: Vec<crate::player::js_lingo_loader::JsScriptRegistration>,
    pub(crate) pending_notifications: CastNotificationOutbox,
    pub name_symbols: Rc<[Symbol]>,
    pub preload_mode: u16,
    pub capital_x: bool,
    pub dir_version: u16,
    /// Offset to adjust bitmap clutId from Config-based to MCsL-based member numbering.
    pub palette_id_offset: i16,
    /// Lazy lowercased-name → lowest member number index for `find_member_by_name`.
    /// Built on first lookup and reused until a member is inserted/removed/renamed
    /// (see `invalidate_name_index`). Replaces an O(members) linear scan per call —
    /// the Habbo preloader's `FindCastNumber` hammers name lookups in tight loops.
    pub name_index: RefCell<Option<FxHashMap<String, u32>>>,
    /// Director Fmap/VWFM font-table snapshot: font_id → font name (e.g.
    /// "Arial", "Arial Bold", "Arial Italic"). Kept on the cast lib so
    /// per-run `font_id`s from STXT can be resolved to actual names at
    /// runtime — `.font` and `.fontStyle` chunk getters need this to
    /// distinguish bold variants from italic variants. Bit-flag heuristics
    /// (e.g. `font_id & 0x8000 = bold`) don't generalise — different movies
    /// pack the table differently and the only authoritative mapping is
    /// the file's own font table.
    pub font_table: HashMap<u16, String>,
}

impl CastLib {
    pub(crate) fn test_external(number: u32, preload_mode: u16) -> Self {
        Self {
            name: format!("external-{number}"),
            file_name: format!("external-{number}.cct"),
            number,
            is_external: true,
            state: CastLibState::None,
            pending_load: None,
            lctx: None,
            members: FxHashMap::default(),
            scripts: FxHashMap::default(),
            pending_js_registrations: Vec::new(),
            pending_notifications: CastNotificationOutbox::default(),
            name_symbols: Rc::from(Vec::<Symbol>::new()),
            preload_mode,
            capital_x: false,
            dir_version: 0,
            palette_id_offset: 0,
            name_index: RefCell::new(None),
            font_table: HashMap::new(),
        }
    }

    /// Take decoded JS-Lingo registrations out of the cast borrow. Callers
    /// must install these through `js_lingo_loader::register_js_script` while
    /// holding the owning `RuntimeSessionHandle`, before exposing the cast's
    /// scripts to execution.
    pub(crate) fn take_js_registrations(
        &mut self,
    ) -> Vec<crate::player::js_lingo_loader::JsScriptRegistration> {
        std::mem::take(&mut self.pending_js_registrations)
    }

    pub(crate) fn take_notifications(&mut self) -> Vec<CastNotification> {
        self.pending_notifications.drain()
    }

    /// Install every registration decoded by `insert_member` through short
    /// session borrows. The queue is removed before JS initialization begins,
    /// so the interpreter can re-enter the owning session without retaining a
    /// mutable cast borrow; unprocessed registrations are restored on error.
    pub(crate) fn install_pending_js_registrations(
        session: crate::player::session::RuntimeSessionHandle,
        player_id: crate::player::session::PlayerId,
        owner: crate::player::ownership::OwnerToken,
        cast_lib: u32,
    ) -> Result<(), crate::player::ScriptError> {
        let registrations = session
            .borrow_mut()
            .with_player(player_id, |context| {
                if !owner.is_arena_live() || !owner.same_identity(&context.player.owner) {
                    return Err(crate::player::ScriptError::new_code(
                        crate::player::ScriptErrorCode::InvalidReference,
                        "stale or foreign Director player".to_owned(),
                    ));
                }
                let index = if cast_lib == 0 {
                    0
                } else {
                    cast_lib as usize - 1
                };
                let cast = context
                    .player
                    .movie
                    .cast_manager
                    .casts
                    .get_mut(index)
                    .ok_or_else(|| {
                        crate::player::ScriptError::new(format!("Cast not found: {}", cast_lib))
                    })?;
                Ok(cast.take_js_registrations())
            })
            .ok_or_else(|| {
                crate::player::ScriptError::new("Director player was removed".to_owned())
            })??;
        let mut remaining = registrations.into_iter();
        while let Some(registration) = remaining.next() {
            let result = crate::player::js_lingo_loader::register_js_script(
                session.clone(),
                player_id,
                owner.clone(),
                registration,
            );
            if let Err(error) = result {
                let message = error.message;
                let unprocessed: Vec<_> = remaining.collect();
                let restore = session.borrow_mut().with_player(player_id, |context| {
                    if !owner.is_arena_live() || !owner.same_identity(&context.player.owner) {
                        return false;
                    }
                    let index = if cast_lib == 0 {
                        0
                    } else {
                        cast_lib as usize - 1
                    };
                    let Some(cast) = context.player.movie.cast_manager.casts.get_mut(index) else {
                        return false;
                    };
                    cast.pending_js_registrations.extend(unprocessed);
                    true
                });
                if !restore.unwrap_or(false) {
                    return Err(crate::player::ScriptError::new_code(
                        crate::player::ScriptErrorCode::InvalidReference,
                        message.clone(),
                    ));
                }
                return Err(crate::player::ScriptError::new(message));
            }
        }
        Ok(())
    }

    pub fn max_member_id(&self) -> u32 {
        *self.members.keys().max().unwrap_or(&0)
    }

    /// Lowest unused member number in this cast lib — the slot Director's
    /// `new(type, castLib(n))` allocates. Member numbers are 1-based.
    ///
    /// Returns 0 ONLY when the cast is genuinely full; callers must treat that
    /// as an error rather than creating a member at slot 0 (see `TypeHandlers::new`).
    ///
    /// This used to scan a hardcoded `1..5000` window and return 0 the moment
    /// every slot in it was taken. Habbo v31's dynamic-download bin cast holds
    /// 4999 members, so it exhausted that window exactly: `new()` then created
    /// each member AT slot 0, whose `.number` encodes to `(castLib-1) << 16`
    /// (131072 for the bin at cast 3) with a zero member index. Habbo's
    /// Resource Manager keys its name -> number map on `member.number`, so
    /// every downloaded furni member registered the SAME 131072 and clobbered
    /// slot 0 in turn; `member(getmemnum(name))` then resolved to whichever
    /// member landed there last — a field — surfacing as "Cannot set castMember
    /// prop paletteRef for field" and leaving the catalogue product strip stuck
    /// on loading placeholders.
    ///
    /// The 5000 ceiling was arbitrary (its own comment asked where it came
    /// from). Director's documented limit is 32,000 members per cast library;
    /// the hard bound here is the 16 bits `get_cast_slot_number` has for the
    /// member index, so scan the whole encodable range.
    pub fn first_free_member_id(&self) -> u32 {
        for i in 1..=0xFFFFu32 {
            if !self.members.contains_key(&i) {
                return i;
            }
        }
        0
    }

    pub fn remove_member(&mut self, number: u32) {
        // TODO remove from movie script cache
        self.members.remove(&number);
        self.scripts.remove(&number);
        self.invalidate_name_index();
        self.pending_notifications
            .push(CastNotification::CastMemberListChanged(self.number));
    }

    /// Reserve a load synchronously. The retained capability prevents a
    /// replacement cast from accepting a stale completion with the same slot.
    pub fn prepare_load(
        &mut self,
        owner: OwnerKey,
        base_path: Option<&url::Url>,
        override_base_path: Option<&str>,
    ) -> Option<CastLoadRequest> {
        if self.file_name.is_empty() || self.state == CastLibState::Loading {
            return None;
        }
        let capability = Arc::new(CastLoadCapability);
        self.pending_load = Some(capability.clone());
        self.state = CastLibState::Loading;
        let resolved_url = resolve_preload_url(&self.file_name, base_path, override_base_path);
        let resolved_url = resolved_url.to_string();
        Some(CastLoadRequest {
            owner,
            cast_number: self.number,
            file_name: self.file_name.clone(),
            requested_url: resolved_url.clone(),
            source_path: self.file_name.clone(),
            cache_key: resolved_url.clone().into(),
            capability,
        })
    }

    /// Reserve a FileName assignment load.  Property assignments replace an
    /// older reservation for the same cast slot; the retained capability on
    /// that older request then makes its eventual completion stale.  This
    /// matches the historical setter, which changed the filename before
    /// beginning its preload and awaited that particular load.
    pub fn prepare_property_load(
        &mut self,
        owner: OwnerKey,
        base_path: Option<&url::Url>,
        override_base_path: Option<&str>,
    ) -> PropertyLoadPreparation {
        if self.file_name.is_empty() {
            self.pending_load = None;
            if self.state == CastLibState::Loading {
                self.state = CastLibState::None;
            }
            return PropertyLoadPreparation::Empty;
        }
        let capability = Arc::new(CastLoadCapability);
        self.pending_load = Some(capability.clone());
        self.state = CastLibState::Loading;
        let resolved_url =
            resolve_preload_url(&self.file_name, base_path, override_base_path).to_string();
        PropertyLoadPreparation::Request(CastLoadRequest {
            owner,
            cast_number: self.number,
            file_name: self.file_name.clone(),
            requested_url: resolved_url.clone(),
            source_path: self.file_name.clone(),
            cache_key: resolved_url.into(),
            capability,
        })
    }

    /// Apply a property-purpose cache hit using the original raw filename as
    /// the DirectorFile name/base-path input.  The historical async preload
    /// looked up `dir_cache` by raw `fileName`, unlike the normalized network
    /// completion key used by the session preload queue.
    pub fn apply_cached_property_file(
        &mut self,
        request: &CastLoadRequest,
        owner: OwnerKey,
        file: Rc<DirectorFile>,
        bitmap_manager: &mut BitmapManager,
        symbols: &mut SymbolTable,
        outbox: &mut CastNotificationOutbox,
    ) -> bool {
        if request.owner != owner
            || request.cast_number != self.number
            || self
                .pending_load
                .as_ref()
                .map_or(true, |cap| !Arc::ptr_eq(cap, &request.capability))
        {
            return false;
        }
        self.pending_load = None;
        self.load_from_dir_file(
            &file,
            request.source_path(),
            bitmap_manager,
            symbols,
            outbox,
        );
        true
    }

    /// Apply a fetched completion synchronously after owner/capability checks.
    pub fn apply_load_result(
        &mut self,
        request: &CastLoadRequest,
        result: CastLoadResult,
        owner: OwnerKey,
        bitmap_manager: &mut BitmapManager,
        dir_cache: &mut HashMap<Box<str>, Rc<DirectorFile>>,
        symbols: &mut SymbolTable,
        outbox: &mut CastNotificationOutbox,
    ) -> bool {
        if request.owner != owner
            || request.cast_number != self.number
            || !Arc::ptr_eq(&request.capability, &result.capability)
            || self
                .pending_load
                .as_ref()
                .map_or(true, |cap| !Arc::ptr_eq(cap, &request.capability))
        {
            return false;
        }
        self.pending_load = None;
        let bytes = match result.bytes {
            Ok(bytes) => bytes,
            Err(_) => {
                self.state = CastLibState::None;
                return true;
            }
        };
        let resolved_url = match Url::parse(&result.resolved_url) {
            Ok(url) => url,
            Err(_) => {
                self.state = CastLibState::None;
                return true;
            }
        };
        let file = match read_director_file_bytes(
            &bytes,
            resolved_url.as_str(),
            &get_base_url(&resolved_url).to_string(),
        ) {
            Ok(file) => file,
            Err(_) => {
                self.state = CastLibState::None;
                return true;
            }
        };
        let file = Rc::new(file);
        dir_cache.insert(request.cache_key.clone(), file.clone());
        let file = dir_cache
            .get(&request.cache_key)
            .expect("cast file inserted synchronously");
        self.load_from_dir_file(file, &result.resolved_url, bitmap_manager, symbols, outbox);
        true
    }

    /// Cancel only the currently reserved load represented by `request`.
    /// Capability identity prevents an old completion from canceling a
    /// replacement request for the same cast slot.
    pub(crate) fn cancel_load(&mut self, request: &CastLoadRequest) -> bool {
        if request.cast_number != self.number
            || self
                .pending_load
                .as_ref()
                .map_or(true, |cap| !Arc::ptr_eq(cap, &request.capability))
        {
            return false;
        }
        self.pending_load = None;
        if self.state == CastLibState::Loading {
            self.state = CastLibState::None;
        }
        true
    }

    pub(crate) fn is_load_current(&self, request: &CastLoadRequest) -> bool {
        request.cast_number == self.number
            && self
                .pending_load
                .as_ref()
                .is_some_and(|cap| Arc::ptr_eq(cap, &request.capability))
    }

    /// Complete a cache hit from the immutable snapshot retained by the
    /// reservation. The cache may be replaced while the reservation waits;
    /// this snapshot keeps the application deterministic.
    pub fn apply_cached_file(
        &mut self,
        request: &CastLoadRequest,
        owner: OwnerKey,
        file: Rc<DirectorFile>,
        bitmap_manager: &mut BitmapManager,
        symbols: &mut SymbolTable,
        outbox: &mut CastNotificationOutbox,
    ) -> bool {
        if request.owner != owner
            || request.cast_number != self.number
            || self
                .pending_load
                .as_ref()
                .map_or(true, |cap| !Arc::ptr_eq(cap, &request.capability))
        {
            return false;
        }
        self.pending_load = None;
        self.load_from_dir_file(&file, &file.file_name, bitmap_manager, symbols, outbox);
        true
    }

    pub fn find_member_by_number(&self, number: u32) -> Option<&CastMember> {
        self.members.get(&number)
    }

    pub fn find_mut_member_by_number(&mut self, number: u32) -> Option<&mut CastMember> {
        self.members.get_mut(&number)
    }

    /// Drop the lazily-built name index. Called whenever the member set or a
    /// member name changes, so the next `find_member_by_name` rebuilds it.
    pub fn invalidate_name_index(&self) {
        self.name_index.replace(None);
    }

    pub fn find_member_by_name(&self, name: &str) -> Option<&CastMember> {
        // An UNNAMED member cannot be addressed by name. Director identifies members
        // by name or by number, and a member with no name is reachable only by number,
        // so an empty query must never match — and unnamed members must never enter
        // the name index, or they become the match for `member("")`.
        //
        // AreaZero's Sound Manager does `member(tSoundName)` where tSoundName comes
        // straight from data that carries 39 `#sound: ""` entries; its own guard only
        // rejects VOID, not EMPTY. Indexing unnamed members made `member("")` resolve
        // to the lowest-numbered unnamed member — a SCRIPT — which then failed with
        // "Script members don't support property duration" and flooded the audio path
        // with non-sound members ("Failed to create source AudioBuffer").
        if name.is_empty() {
            return None;
        }
        // Director returns the lowest-numbered member when duplicates exist in the
        // same cast. Build (once) a lowercased-name → lowest-number index so repeated
        // lookups are O(1) instead of an O(members) scan per call.
        if self.name_index.borrow().is_none() {
            let mut index: FxHashMap<String, u32> = FxHashMap::default();
            for member in self.members.values() {
                if member.name.is_empty() {
                    continue;
                }
                let key = member.name.to_ascii_lowercase();
                index
                    .entry(key)
                    .and_modify(|n| {
                        if member.number < *n {
                            *n = member.number;
                        }
                    })
                    .or_insert(member.number);
            }
            self.name_index.replace(Some(index));
        }

        let number = {
            let idx_ref = self.name_index.borrow();
            let idx = idx_ref.as_ref().unwrap();
            // Avoid allocating a lowercased key on every lookup: most callers
            // (Habbo's normalizeCastName output) already pass a lowercase name,
            // so probe the index directly with `&str` and only allocate when the
            // name actually contains uppercase ASCII.
            if name.bytes().any(|b| b.is_ascii_uppercase()) {
                let lookup = name.to_ascii_lowercase();
                idx.get(lookup.as_str()).copied()
            } else {
                idx.get(name).copied()
            }
        };
        number.and_then(|n| self.members.get(&n))
    }

    fn clear(&mut self) {
        self.clear_with_outbox(None);
    }

    fn clear_with_outbox(&mut self, mut outbox: Option<&mut CastNotificationOutbox>) {
        // Clear regardless of state. The previous early-return-when-not-Loaded
        // guard left stale members in place when a swap-in-place reload went
        // through the network path of `preload`: that path sets
        // `state = Loading` *before* awaiting the fetch, and by the time
        // `load_from_dir_file` calls `clear()` the guard short-circuited so
        // the OLD cast's members survived. `apply_cast_def` then merged the
        // new cast on top, leaving any slot the new cast didn't redefine
        // pinned to the previous cast's content (e.g. Coke Studios' first-time
        // swap from one public studio to another bled walls/floor/decor from
        // the previously visited studio).
        if self.members.is_empty()
            && self.scripts.is_empty()
            && self.lctx.is_none()
            && self.state == CastLibState::None
        {
            return;
        }
        self.members.clear();
        self.scripts.clear();
        self.pending_js_registrations.clear();
        self.lctx = None;
        self.state = CastLibState::None;
        self.pending_load = None;
        self.invalidate_name_index();

        if let Some(outbox) = outbox.as_deref_mut() {
            outbox.push(CastNotification::CastMemberListChanged(self.number));
        } else {
            self.pending_notifications
                .push(CastNotification::CastMemberListChanged(self.number));
        }
    }

    fn set_name(&mut self, name: String) {
        self.set_name_with_outbox(name, None);
    }

    fn set_name_with_outbox(
        &mut self,
        name: String,
        mut outbox: Option<&mut CastNotificationOutbox>,
    ) {
        if name != self.name {
            self.name = name;
            if let Some(outbox) = outbox.as_deref_mut() {
                outbox.push(CastNotification::CastNameChanged(self.number));
            } else {
                self.pending_notifications
                    .push(CastNotification::CastNameChanged(self.number));
            }
        }
    }

    pub fn set_prop(
        &mut self,
        prop: Symbol,
        value: Datum,
        datums: &DatumAllocator,
        symbols: &SymbolTable,
        outbox: Option<&mut CastNotificationOutbox>,
    ) -> Result<(), ScriptError> {
        // TODO
        match prop.into_builtin() {
            Some(BuiltInSymbol::PreloadMode) => {
                self.preload_mode = value.int_value()? as u16;
            }
            Some(BuiltInSymbol::Name) => {
                self.set_name_with_outbox(value.string_value(symbols)?, outbox);
            }
            Some(BuiltInSymbol::FileName) => {
                self.file_name = value.string_value(symbols)?;
            }
            _ => {
                return Err(ScriptError::new(format!(
                    "Cannot set castLib property {}",
                    symbols
                        .display(&prop)
                        .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
                )));
            }
        };
        Ok(())
    }

    pub fn get_prop(&self, prop: Symbol, symbols: &SymbolTable) -> Result<Datum, ScriptError> {
        match prop.into_builtin() {
            Some(BuiltInSymbol::PreloadMode) => Ok(Datum::Int(self.preload_mode as i32)),
            Some(BuiltInSymbol::FileName) => {
                // Only return the fileName if the cast is actually loaded.
                // External casts with preloadMode=0 ("When Needed") have file_name set
                // in the movie structure but aren't loaded yet. Scripts like PreloadCast
                // compare castLib.fileName to decide whether to download — returning the
                // configured name for an unloaded cast would skip the download.
                if self.is_external && self.state == CastLibState::None {
                    Ok(Datum::String(String::new()))
                } else {
                    Ok(Datum::String(self.file_name.clone()))
                }
            }
            Some(BuiltInSymbol::Number) => Ok(Datum::Int(self.number as i32)),
            Some(BuiltInSymbol::Name) => Ok(Datum::String(self.name.clone())),
            Some(BuiltInSymbol::NumberOfCastMembers | BuiltInSymbol::NumberOfMembers) => {
                // Director semantics: the highest member slot number in use,
                // not the population count. Casts routinely have gaps, and
                // Lingo code like `repeat with i = 1 to the number of
                // castMembers of castLib "X"` relies on this to reach every
                // populated slot (including dynamically created members at
                // high numbers) — otherwise cleanup loops silently skip them.
                Ok(Datum::Int(self.max_member_id() as i32))
            }
            _ => Err(ScriptError::new(format!(
                "Cannot get castLib property {}",
                symbols
                    .display(&prop)
                    .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
            ))),
        }
    }

    fn load_from_dir_file(
        &mut self,
        file: &DirectorFile,
        load_file_name: &str,
        bitmap_manager: &mut BitmapManager,
        symbols: &mut SymbolTable,
        outbox: &mut CastNotificationOutbox,
    ) {
        self.clear_with_outbox(Some(&mut *outbox));
        // TODO file.parseScripts

        self.file_name = load_file_name.to_owned();
        self.state = CastLibState::Loaded;
        if self.name.is_empty() {
            self.set_name_with_outbox(
                get_basename_no_extension(load_file_name),
                Some(&mut *outbox),
            );
        }
        if let Some(cast_def) = file.casts.first() {
            log::debug!(
                "Applying cast def to castLib {} ('{}'): {} members",
                self.number,
                self.name,
                cast_def.members.len()
            );
            self.apply_cast_def_with_outbox(
                file,
                cast_def,
                bitmap_manager,
                &file.font_table,
                symbols,
                Some(&mut *outbox),
            );
        } else {
            log_i(
                format_args!(
                    "No cast def found in file {} for castLib {} ('{}')",
                    load_file_name, self.number, self.name
                )
                .to_string()
                .as_str(),
            );
        }
    }

    pub fn apply_cast_def(
        &mut self,
        file: &DirectorFile,
        cast_def: &CastDef,
        bitmap_manager: &mut BitmapManager,
        font_table: &HashMap<u16, String>,
        symbols: &mut SymbolTable,
    ) {
        self.apply_cast_def_with_outbox(file, cast_def, bitmap_manager, font_table, symbols, None);
    }

    fn apply_cast_def_with_outbox(
        &mut self,
        _: &DirectorFile,
        cast_def: &CastDef,
        bitmap_manager: &mut BitmapManager,
        font_table: &HashMap<u16, String>,
        symbols: &mut SymbolTable,
        mut outbox: Option<&mut CastNotificationOutbox>,
    ) {
        self.lctx = cast_def.lctx.clone();
        // AUTHORITATIVE: a cast's own name table claims the global display
        // spelling for its names, overriding a builtin that interned the same
        // name in different casing at startup. Director has no builtin spelling
        // table seeded before the movie, so there the movie is always the first
        // writer; `init_builtin_symbols()` makes us the first writer instead.
        // Habbo v31: the XML builtin `nodeName` claimed the spelling, so the
        // movie's `#nodename` reported as "nodeName" and the catalogue's
        // `tdata.getaProp("nodename")` missed ("Malformed node data nodeName").
        self.name_symbols = self
            .lctx
            .as_ref()
            .map(|lctx| {
                Rc::from(
                    lctx.names
                        .iter()
                        .map(|n| symbols.intern_authoritative(n))
                        .collect::<Vec<_>>(),
                )
            })
            .unwrap_or_else(|| Rc::from(Vec::<Symbol>::new()));
        self.capital_x = cast_def.capital_x;
        self.dir_version = cast_def.dir_version;
        self.palette_id_offset = cast_def.palette_id_offset;
        self.font_table = font_table.clone();
        self.state = CastLibState::Loaded;
        let mut member_ids: Vec<u32> = cast_def.members.keys().copied().collect();
        member_ids.sort_unstable();
        for id in member_ids {
            let member_def = &cast_def.members[&id];
            self.insert_member(
                id,
                CastMember::from(
                    self.number,
                    id,
                    member_def,
                    &self.lctx,
                    bitmap_manager,
                    self.dir_version,
                    self.palette_id_offset,
                    font_table,
                    symbols,
                ),
                symbols,
            );
            let slot = CastMemberRefHandlers::get_cast_slot_number(self.number, id);
            if let Some(outbox) = outbox.as_deref_mut() {
                outbox.push(CastNotification::CastMemberNameChanged(slot));
            }
        }
        if let Some(outbox) = outbox.as_deref_mut() {
            outbox.push(CastNotification::CastMemberListChanged(self.number));
        } else {
            self.pending_notifications
                .push(CastNotification::CastMemberListChanged(self.number));
        }
        unsafe {
            let player_mut = &mut crate::player::player_mut();

            player_mut.movie.cast_manager.clear_movie_script_cache();
            player_mut.movie.cast_manager.invalidate_member_name_cache();
            player_mut
                .movie
                .cast_manager
                .load_fonts_into_manager(&mut player_mut.font_manager);
        };
    }

    pub fn insert_member(&mut self, number: u32, member: CastMember, symbols: &mut SymbolTable) {
        // Which lctx script should we register under this member's slot, and as
        // what type? A `Script` cast member registers its own script. A non-Script
        // member (Field, Text, Bitmap, Button, Shape) may carry an ATTACHED script
        // via `member_info.header.script_id` — Director lets `script("name")` and
        // `new(script(...))` resolve to that attached script, and a movie can store
        // a parent script AS a Field cast member (SpongeBob "JellyFishin'" stores
        // its "hero parent" parent script as a Field). Register it at the member
        // slot so `get_script_for_member(number)` finds it. (Member BEHAVIOR scripts
        // for mouse events are dispatched separately via get_behavior_script_from_lctx.)
        let registration: Option<(u32, ScriptType)> = match &member.member_type {
            CastMemberType::Script(s) => Some((s.script_id, s.script_type)),
            _ => member.get_script_id().map(|sid| (sid, ScriptType::Parent)),
        };

        if let Some((reg_script_id, reg_script_type)) = registration {
            let script_def = self
                .lctx
                .as_ref()
                .and_then(|lctx| lctx.scripts.get(&reg_script_id));

            if let Some(script_def) = script_def {
                let mut handler_names = Vec::new();
                let mut handler_names_raw = Vec::new();
                let mut handler_name_map = FxHashMap::default();
                let names = &self.lctx.as_ref().unwrap().names;
                for (idx, handler) in script_def.handlers.iter().enumerate() {
                    // name_id 0xFFFF (and any out-of-range id) marks an
                    // anonymous handler slot — Director leaves these in the
                    // handler vector (netjack D4 has one). It must still occupy
                    // its position: `LocalCall` / `get_own_handler_ref_at`
                    // index handler_names BY POSITION, so skipping would
                    // misalign every later handler. Give it a unique synthetic
                    // name (registered in the map too) so it stays callable by
                    // index without indexing past the names table.
                    let handler_name = match names.get(handler.name_id as usize) {
                        Some(n) => n.clone(),
                        None => format!("__anon_handler_{}", idx),
                    };
                    let handler_name_symbol = symbols.intern(&handler_name);
                    handler_name_map.insert(handler_name_symbol.clone(), Rc::new(handler.clone()));
                    handler_names.push(handler_name_symbol);
                    handler_names_raw.push(handler_name);
                }

                let property_names = script_def
                    .property_name_ids
                    .iter()
                    .filter_map(|id| names.get(*id as usize).map(|n| symbols.intern(n)));
                let mut properties = FxHashMap::default();
                for name in property_names {
                    properties.insert(name, DatumRef::Void);
                }

                let script = Script {
                    member_ref: cast_member_ref(self.number as i32, number as i32),
                    name: (&member.name).to_owned(),
                    chunk: script_def.clone(),
                    script_type: reg_script_type,
                    handlers: handler_name_map,
                    handler_names,
                    handler_names_raw,
                    properties: RefCell::new(properties),
                };
                // JS Lingo diagnostic: decode and disassemble each XDR-wrapped JSScript
                // in the literal data area. Phase 1 — read-only; translator hook lands later.
                if let Some(registration) =
                    crate::player::js_lingo_loader::diagnose_js_script(&script)
                {
                    self.pending_js_registrations.push(registration);
                }
                self.scripts.insert(number, Rc::new(script));
            }
        } else if let CastMemberType::Palette(_) = &member.member_type {
            reserve_player_mut(|player| {
                player.movie.cast_manager.invalidate_palette_cache();
            });
        }

        self.members.insert(number, member);
        self.invalidate_name_index();
    }

    pub fn create_member_at(
        &mut self,
        number: u32,
        member_type: &str,
        bitmap_manager: &mut BitmapManager,
        symbols: &mut SymbolTable,
    ) -> Result<CastMemberRef, ScriptError> {
        // Director symbols are case-insensitive (`new(#vectorShape)` ==
        // `new(#vectorshape)`), so match member-type names through `match_ci!`.
        let member = match_ci!(member_type, {
            "field" => Ok(CastMember::new(
                number,
                CastMemberType::Field(FieldMember::new()),
            )),
            "text" => Ok(CastMember::new(
                number,
                CastMemberType::Text(TextMember::new()),
            )),
            "bitmap" => {
                let bitmap = Bitmap::new(
                    0,
                    0,
                    32,
                    32,
                    0,
                    PaletteRef::BuiltIn(BuiltInPalette::GrayScale),
                );
                let bitmap_ref = bitmap_manager.add_bitmap(bitmap);
                Ok(CastMember::new(
                    number,
                    CastMemberType::Bitmap(BitmapMember {
                        image_ref: bitmap_ref,
                        reg_point: (0, 0),
                        script_id: 0,
                        member_script_ref: None,
                        info: BitmapInfo::default(),
                    }),
                ))
            },
            "palette" => Ok(CastMember::new(
                number,
                CastMemberType::Palette(PaletteMember::new()),
            )),
            // `new(#vectorShape)` creates an empty vector shape; the script
            // populates vertices via `addVertex` and sets fill/stroke/gradient
            // props (Director 11.5 Scripting Dictionary). spectral-wizard's
            // parent_grad builds gradient backgrounds this way at runtime.
            "vectorshape" => Ok(CastMember::new(
                number,
                CastMemberType::VectorShape(VectorShapeMember::new()),
            )),
            // `new(#sound, castLib)` creates an empty sound member; its media is
            // populated later, typically via `importFileInto` (Director 11.5
            // Scripting Dictionary, `new()` / `importFileInto()`). Habbo's
            // Download Manager relies on this for streamed trax samples.
            "sound" => Ok(CastMember::new(
                number,
                CastMemberType::Sound(SoundMember {
                    info: SoundInfo::default(),
                    sound: SoundChunk::default(),
                    cue_point_times: Vec::new(),
                    cue_point_names: Vec::new(),
                }),
            )),
            "script" => Ok(CastMember::new(
                number,
                CastMemberType::Script(ScriptMember {
                    script_id: 0,
                    script_type: ScriptType::Movie,
                    name: String::new(),
                }),
            )),
            // `new(#flash)` creates an empty Flash cast member; the script then
            // points it at a SWF, typically by setting `.linked = TRUE` and
            // `.pathName = "http://…/foo.swf"`, or by assigning preloaded bytes
            // (Director 11.5 Scripting Dictionary — `new()`, `#flash` member).
            // Neopets' DGS loader (`mainClass.showPreLoader`) uses this to host
            // the downloaded preloader SWF. Empty until its source is assigned.
            "flash" => Ok(CastMember::new(
                number,
                CastMemberType::Flash(FlashMember {
                    data: Vec::new(),
                    reg_point: (0, 0),
                    flash_info: None,
                }),
            )),
            // `new(#movie)` creates an empty Linked Movie member; the script
            // links it to an external .dir/.dcr via `member.fileName = <url>`
            // and plays it by assigning the member to a sprite (Director 11.5
            // Scripting Dictionary "Linked Movie"). Neopets' DGS loader
            // (`showAndStartShockwaveGame`) uses this to run the downloaded game.
            "movie" => Ok(CastMember::new(
                number,
                CastMemberType::Movie(MovieMember::new()),
            )),
            // `new(#physics)` creates an empty Physics cast member. `#physics` is
            // a documented member type (Director 11.5 Scripting Dictionary,
            // `type (Member)` — the value list includes `#physics`), supplied by
            // the AGEIA Physics Xtra that ships with 11.5. The script then calls
            // `member.init(...)` / `createRigidBody(...)` on it, exactly as the
            // authored Physics members in Agent Free Ride are used. AreaZero's
            // `[M] Member.createMember` builds its "MenuCharacter_Physics" member
            // this way instead of authoring it in the cast.
            "physics" => Ok(CastMember::new(
                number,
                CastMemberType::PhysXPhysics(crate::player::cast_member::PhysXPhysicsMember {
                    state: crate::player::cast_member::PhysXPhysicsState::default(),
                }),
            )),
            _ => Err(ScriptError::new(format!(
                "Cannot create member of type {}",
                member_type
            ))),
        })?;
        self.insert_member(number, member, symbols);
        self.pending_notifications
            .push(CastNotification::CastMemberListChanged(self.number));
        Ok(cast_member_ref(self.number as i32, number as i32))
    }

    pub fn get_script_for_member(&self, number: u32) -> Option<&Rc<Script>> {
        // Direct path: `number` is a cast-member slot that holds a Script
        // member — this is how scripts authored as standalone behaviors are
        // registered (see `insert_member`, which inserts `scripts[number]`).
        if let Some(script) = self.scripts.get(&number) {
            return Some(script);
        }

        // Fallback: `number` is an lctx-script-id (the value stored in
        // `member_info.header.script_id` for non-Script members like Field
        // and Text). In D11+ movies this id may NOT equal the cast-member
        // slot — e.g. a field with `header.script_id=10` may have its
        // actual script cast member elsewhere. Walk the cast looking for
        // a Script-type member whose own `script_id` matches, then return
        // its registered script.
        for (slot, member) in &self.members {
            if let CastMemberType::Script(script_member) = &member.member_type {
                if script_member.script_id == number {
                    return self.scripts.get(slot);
                }
            }
        }
        None
    }

    pub fn get_behavior_script_from_lctx(
        &mut self,
        script_id: u32,
        symbols: &mut SymbolTable,
    ) -> Option<Rc<Script>> {
        // Use an offset to avoid collision with cast member numbers
        // Behavior scripts are stored at script_id + 1000000
        let cache_key = script_id + 1000000;

        // Check if already cached
        if let Some(cached) = self.scripts.get(&cache_key) {
            return Some(cached.clone());
        }

        // Get script chunk from lctx.scripts
        let script_chunk = self.lctx.as_ref()?.scripts.get(&script_id)?;

        // Build handler map
        let mut handler_names = Vec::new();
        let mut handler_names_raw = Vec::new();
        let mut handler_name_map = FxHashMap::default();
        let names = &self.lctx.as_ref().unwrap().names;
        for (idx, handler) in script_chunk.handlers.iter().enumerate() {
            // Anonymous handler slots (name_id 0xFFFF / out of range) must keep
            // their position — LocalCall indexes handler_names by position.
            // Give them a unique synthetic name instead of skipping.
            let handler_name = match names.get(handler.name_id as usize) {
                Some(n) => n.clone(),
                None => format!("__anon_handler_{}", idx),
            };
            let handler_name_symbol = symbols.intern(&handler_name);
            handler_name_map.insert(handler_name_symbol.clone(), Rc::new(handler.clone()));
            handler_names.push(handler_name_symbol);
            handler_names_raw.push(handler_name);
        }

        // Build properties
        let property_names = script_chunk
            .property_name_ids
            .iter()
            .filter_map(|id| names.get(*id as usize).map(|n| symbols.intern(n)));
        let mut properties = FxHashMap::default();
        for name in property_names {
            properties.insert(name, DatumRef::Void);
        }

        let script = Rc::new(Script {
            member_ref: cast_member_ref(self.number as i32, cache_key as i32),
            name: format!("BehaviorScript_{}", script_id),
            chunk: script_chunk.clone(),
            script_type: ScriptType::Member,
            handlers: handler_name_map,
            handler_names,
            handler_names_raw,
            properties: RefCell::new(properties),
        });

        // Cache it with the offset key
        self.scripts.insert(cache_key, script.clone());

        Some(script)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Copy)]
pub struct CastMemberRef {
    pub cast_lib: i32,
    pub cast_member: i32,
}

pub const INVALID_CAST_MEMBER_REF: CastMemberRef = CastMemberRef {
    cast_lib: -1,
    cast_member: -1,
};
pub const NULL_CAST_MEMBER_REF: CastMemberRef = CastMemberRef {
    cast_lib: 0,
    cast_member: 0,
};

pub fn cast_member_ref(cast_lib: i32, cast_member: i32) -> CastMemberRef {
    CastMemberRef {
        cast_lib,
        cast_member,
    }
}

impl CastMemberRef {
    pub fn is_valid(&self) -> bool {
        // The NULL ref (0, 0) is "no member" — it is what an EMPTY sprite
        // channel reports and what `member(0, 0)` builds — so it is not a valid
        // member any more than the INVALID (-1, -1) sentinel is. Without this,
        // a channel cleared with `sprite.member = member(0, 0)` still counted
        // as occupied: it kept hit-testing, so an invisible cleared overlay
        // swallowed clicks meant for the sprite beneath it (LEGO Supersonic's
        // menu stopped responding).
        if self.cast_lib == NULL_CAST_MEMBER_REF.cast_lib
            && self.cast_member == NULL_CAST_MEMBER_REF.cast_member
        {
            return false;
        }
        self.cast_lib != INVALID_CAST_MEMBER_REF.cast_lib
            && self.cast_member != INVALID_CAST_MEMBER_REF.cast_member
    }
}

#[cfg(test)]
mod name_index_tests {
    use super::*;
    use crate::director::chunks::{handler::HandlerDef, script::ScriptChunk};
    use crate::player::cast_member::FieldMember;
    use crate::player::script::Script;
    use crate::player::ScriptErrorCode;

    fn empty_cast() -> CastLib {
        let mut symbols = SymbolTable::new();
        CastLib {
            name: String::new(),
            file_name: String::new(),
            number: 1,
            is_external: false,
            state: CastLibState::Loaded,
            pending_load: None,
            lctx: None,
            members: FxHashMap::default(),
            scripts: FxHashMap::default(),
            pending_js_registrations: Vec::new(),
            pending_notifications: CastNotificationOutbox::default(),
            name_symbols: Rc::from(Vec::<Symbol>::new()),
            preload_mode: 0,
            capital_x: false,
            dir_version: 0,
            palette_id_offset: 0,
            name_index: RefCell::new(None),
            font_table: HashMap::new(),
        }
    }

    fn pending_external_cast(number: u32) -> CastLib {
        let mut cast = empty_cast();
        cast.number = number;
        cast.file_name = format!("external-{number}.cct");
        cast.is_external = true;
        cast.state = CastLibState::None;
        cast
    }

    fn field_member(number: u32, name: &str) -> CastMember {
        // Field members take the simple insert path (no JsApi / script wiring).
        let mut m = CastMember::new(number, CastMemberType::Field(FieldMember::new()));
        m.name = name.to_string();
        m
    }

    #[test]
    fn cast_properties_resolve_through_the_supplied_symbol_table() {
        let mut cast = empty_cast();
        let symbols = SymbolTable::new();
        let datums = DatumAllocator::new(OwnerKey {
            session: 11,
            player: 1,
            generation: 1,
        });
        cast.set_prop(
            Symbol::builtin(BuiltInSymbol::PreloadMode),
            Datum::Int(2),
            &datums,
            &symbols,
            None,
        )
        .unwrap();
        assert!(matches!(
            cast.get_prop(Symbol::builtin(BuiltInSymbol::PreloadMode), &symbols)
                .unwrap(),
            Datum::Int(2)
        ));

        let mut outbox = CastNotificationOutbox::default();
        cast.set_prop(
            Symbol::builtin(BuiltInSymbol::Name),
            Datum::String("renamed".to_owned()),
            &datums,
            &symbols,
            Some(&mut outbox),
        )
        .unwrap();
        assert_eq!(cast.name, "renamed");
        assert_eq!(
            outbox.drain(),
            vec![CastNotification::CastNameChanged(cast.number)]
        );

        let mut foreign_symbols = SymbolTable::new();
        let foreign = foreign_symbols.intern("foreignCastProperty");
        match cast.get_prop(foreign, &symbols) {
            Err(error) => assert_eq!(error.code, ScriptErrorCode::InvalidReference),
            Ok(_) => panic!("foreign property unexpectedly resolved"),
        }
    }

    #[test]
    fn movie_handler_cache_keeps_first_cast_source() {
        fn script(member: u32, name: &str, handler_name: Symbol) -> Rc<Script> {
            let handler = HandlerDef {
                name_id: 0,
                bytecode_array: Vec::new(),
                bytecode_index_map: FxHashMap::default(),
                argument_name_ids: Vec::new(),
                local_name_ids: Vec::new(),
                global_name_ids: Vec::new(),
                compiled_ir: RefCell::new(None),
            };
            let mut handlers = FxHashMap::default();
            handlers.insert(handler_name.clone(), Rc::new(handler));
            Rc::new(Script {
                member_ref: cast_member_ref(1, member as i32),
                name: name.to_owned(),
                chunk: ScriptChunk {
                    script_number: member as u16,
                    literals: Vec::new(),
                    handlers: Vec::new(),
                    property_name_ids: Vec::new(),
                    property_defaults: HashMap::new(),
                },
                script_type: ScriptType::Movie,
                handlers,
                handler_names_raw: vec![name.to_owned()],
                handler_names: vec![handler_name],
                properties: RefCell::new(FxHashMap::default()),
            })
        }

        let mut manager = super::super::cast_manager::CastManager::empty();
        let mut first = empty_cast();
        first
            .scripts
            .insert(2, script(2, "first", Symbol::builtin(BuiltInSymbol::New)));
        let mut second = empty_cast();
        second.number = 2;
        second
            .scripts
            .insert(3, script(3, "second", Symbol::builtin(BuiltInSymbol::New)));
        manager.casts.push(first);
        manager.casts.push(second);

        let handler = Symbol::builtin(BuiltInSymbol::New);
        assert_eq!(manager.movie_handler_ref(handler).unwrap().0.cast_member, 2);
    }

    #[test]
    fn find_by_name_is_case_insensitive_and_lowest_number_wins() {
        let mut cast = empty_cast();
        let mut symbols = SymbolTable::new();
        cast.insert_member(5, field_member(5, "Window"), &mut symbols);
        cast.insert_member(2, field_member(2, "window"), &mut symbols); // duplicate name, lower number
        cast.insert_member(9, field_member(9, "Frame"), &mut symbols);

        // Lowest-numbered match wins; lookup is case-insensitive.
        assert_eq!(
            cast.find_member_by_name("WINDOW").map(|m| m.number),
            Some(2)
        );
        assert_eq!(
            cast.find_member_by_name("window").map(|m| m.number),
            Some(2)
        );
        assert_eq!(cast.find_member_by_name("frame").map(|m| m.number), Some(9));
        assert!(cast.find_member_by_name("missing").is_none());
        // Second lookup hits the cached index and returns the same result.
        assert_eq!(
            cast.find_member_by_name("window").map(|m| m.number),
            Some(2)
        );
    }

    #[test]
    fn index_invalidates_on_rename_and_remove() {
        let mut cast = empty_cast();
        let mut symbols = SymbolTable::new();
        cast.insert_member(3, field_member(3, "Alpha"), &mut symbols);
        assert_eq!(cast.find_member_by_name("alpha").map(|m| m.number), Some(3)); // builds index

        // Rename: mutate the member then invalidate (mirrors the name-setter,
        // which routes through invalidate_member_name_cache -> invalidate_name_index).
        cast.members.get_mut(&3).unwrap().name = "Beta".to_string();
        cast.invalidate_name_index();
        assert!(cast.find_member_by_name("alpha").is_none());
        assert_eq!(cast.find_member_by_name("beta").map(|m| m.number), Some(3));

        // Remove via the map + invalidate (avoid remove_member's JsApi calls in tests).
        cast.members.remove(&3);
        cast.invalidate_name_index();
        assert!(cast.find_member_by_name("beta").is_none());
    }

    #[test]
    fn canceled_load_rejects_late_completion_and_allows_restart() {
        let owner = OwnerKey {
            session: 7,
            player: 3,
            generation: 1,
        };
        let mut cast = pending_external_cast(1);
        let base_path = Url::parse("file:///tmp/").unwrap();
        let request = cast.prepare_load(owner, Some(&base_path), None).unwrap();
        let stale_before_restart = request.complete(
            "file:///external-1.cct".to_owned(),
            Err("canceled".to_owned()),
        );
        assert!(cast.cancel_load(&request));
        assert_eq!(cast.state, CastLibState::None);

        let mut bitmap_manager = BitmapManager::new();
        let mut dir_cache = HashMap::new();
        let mut symbols = SymbolTable::new();
        let mut outbox = CastNotificationOutbox::default();
        assert!(!cast.apply_load_result(
            &request,
            stale_before_restart,
            owner,
            &mut bitmap_manager,
            &mut dir_cache,
            &mut symbols,
            &mut outbox,
        ));

        let replacement = cast.prepare_load(owner, Some(&base_path), None).unwrap();
        assert!(!std::sync::Arc::ptr_eq(
            request.capability(),
            replacement.capability()
        ));
        let stale_after_restart =
            request.complete("file:///external-1.cct".to_owned(), Err("stale".to_owned()));
        assert!(!cast.apply_load_result(
            &replacement,
            stale_after_restart,
            owner,
            &mut bitmap_manager,
            &mut dir_cache,
            &mut symbols,
            &mut outbox,
        ));
        let fresh = replacement.complete(
            "file:///external-1.cct".to_owned(),
            Err("fetch failed".to_owned()),
        );
        assert!(cast.apply_load_result(
            &replacement,
            fresh,
            owner,
            &mut bitmap_manager,
            &mut dir_cache,
            &mut symbols,
            &mut outbox,
        ));
        let duplicate = replacement.complete(
            "file:///external-1.cct".to_owned(),
            Err("duplicate".to_owned()),
        );
        assert!(!cast.apply_load_result(
            &replacement,
            duplicate,
            owner,
            &mut bitmap_manager,
            &mut dir_cache,
            &mut symbols,
            &mut outbox,
        ));
        assert_eq!(cast.state, CastLibState::None);
    }

    #[test]
    fn manager_cancellation_only_removes_exact_preload_requirement() {
        let owner = OwnerKey {
            session: 8,
            player: 4,
            generation: 2,
        };
        let mut manager = super::super::cast_manager::CastManager::empty();
        manager.casts.push(pending_external_cast(1));
        manager.casts.push(pending_external_cast(2));
        let cache = HashMap::new();
        let base_path = Url::parse("file:///tmp/").unwrap();
        let requests = manager.prepare_preload_requests(
            super::super::cast_manager::CastPreloadReason::MovieLoaded,
            owner,
            &cache,
            Some(&base_path),
            None,
        );
        assert_eq!(requests.len(), 2);
        assert_eq!(
            manager.preload_state,
            super::super::cast_manager::CastPreloadState::Loading
        );
        assert!(manager.cancel_preload(&requests[0]));
        assert_eq!(
            manager.preload_state,
            super::super::cast_manager::CastPreloadState::Loading
        );
        assert!(manager.cancel_preload(&requests[1]));
        assert_eq!(
            manager.preload_state,
            super::super::cast_manager::CastPreloadState::Idle
        );

        let replacement_requests = manager.prepare_preload_requests(
            super::super::cast_manager::CastPreloadReason::MovieLoaded,
            owner,
            &cache,
            Some(&base_path),
            None,
        );
        assert_eq!(replacement_requests.len(), 2);
        assert!(!manager.cancel_preload(&requests[0]));
        assert!(!manager.retire_preload_requirement(&requests[0]));
        assert_eq!(
            manager.preload_state,
            super::super::cast_manager::CastPreloadState::Loading
        );
    }
}
