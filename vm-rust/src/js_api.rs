use std::{collections::{HashMap, HashSet}, iter::FromIterator};

use itertools::Itertools;
use js_sys::Array;
use log::debug;
use wasm_bindgen::prelude::*;

use crate::{
    director::{
        chunks::{self, script::ScriptChunk, Chunk},
        enums::ScriptType,
        file::{DirectorFile, get_variable_multiplier},
        lingo::{datum::Datum, decompiler, script::ScriptContext},
        rifx::RIFXReaderContext,
        utils::fourcc_to_string,
    },
    player::{
        allocator::ScriptInstanceAllocatorTrait,
        bitmap::bitmap::PaletteRef,
        cast_lib::{CastMemberRef, PlayerNotification, PlayerNotificationKind},
        cast_member::{CastMember, CastMemberType, ScriptMember},
        datum_formatting::{format_concrete_datum, format_datum, format_float_with_precision, format_numeric_value},
        datum_ref::{DatumId, DatumRef},
        handlers::datum_handlers::cast_member_ref::CastMemberRefHandlers,
        score::get_channel_number_from_index,
        score::Score,
        script::ScriptInstanceId,
        script_ref::ScriptInstanceRef,
        DirPlayer, ScriptError, PLAYER_OPT, owner_key_string,
        session::{PlayerId, RuntimeSessionHandle},
        sprite::{ColorRef, CursorRef},
        symbols::symbol_table::SymbolTable,
        host_events::{
            HostEvent, NativeBehaviorReference, NativeChannelSnapshot, NativeDatumSnapshot,
            NativeDatumValue, NativeMemberSnapshot, NativePlayerNotification,
            NativePlayerNotificationKind, NativeScoreSnapshot, NativeScoreSpan,
            NativeScriptInstanceSnapshot,
        },
    },
    rendering::RENDERER_LOCK,
};

#[derive(Clone)]
pub struct ScoreSpriteSpan {
    pub channel_number: u16,
    pub start_frame: u32,
    pub end_frame: u32,
    pub member_ref: [u16; 2], // [cast_lib, cast_member]
    /// Resolved once here rather than in the UI: the timeline labels clips with
    /// it, and the alternative — shipping cast member lists to JS so it could
    /// look names up — is exactly the wholesale transfer we removed. Empty when
    /// the member is unnamed or missing, and the UI falls back to the numbers.
    pub member_name: String,
}

impl ToJsValue for ScoreSpriteSpan {
    fn to_js_value(&self) -> JsValue {
        let span_map = js_sys::Map::new();
        span_map.str_set("startFrame", &self.start_frame.to_js_value());
        span_map.str_set("endFrame", &self.end_frame.to_js_value());
        span_map.str_set("channelNumber", &self.channel_number.to_js_value());
        span_map.str_set(
            "memberRef",
            &js_sys::Array::of2(
                &JsValue::from(self.member_ref[0]),
                &JsValue::from(self.member_ref[1]),
            ),
        );
        span_map.str_set("memberName", &safe_js_string(&self.member_name));
        span_map.to_js_object().into()
    }
}

fn format_atom_summary(a: &crate::player::js_lingo::xdr::JsAtom) -> String {
    use crate::player::js_lingo::xdr::JsAtom;
    match a {
        JsAtom::Null => "null".into(),
        JsAtom::Void => "void".into(),
        JsAtom::Bool(b) => b.to_string(),
        JsAtom::Int(i) => i.to_string(),
        JsAtom::Double(d) => format!("{}", d),
        JsAtom::String(s) => format!("{:?}", s),
        JsAtom::Function(f) => format!(
            "function {}({})",
            f.name.as_deref().unwrap_or("<anonymous>"),
            f.bindings.iter()
                .filter(|b| b.kind == crate::player::js_lingo::xdr::JsBindingKind::Argument)
                .map(|b| b.name.as_str()).collect::<Vec<_>>().join(", ")
        ),
        JsAtom::Unsupported(t) => format!("<unsupported tag={}>", t),
    }
}

pub fn ascii_safe(string: &str) -> String {
    string
        .chars()
        .map(|c| match c as u32 {
            9 => '\t',
            10 => '\n',
            13 => '\r',
            32..=126 => c,
            _ => '?',
        })
        .collect()
}

pub fn safe_string(s: &str) -> String {
    String::from_utf8_lossy(s.as_bytes()).into_owned()
}

#[cfg(target_arch = "wasm32")]
pub fn safe_js_string(s: &str) -> JsValue {
    JsValue::from_str(&safe_string(s))
}

#[cfg(not(target_arch = "wasm32"))]
pub fn safe_js_string(s: &str) -> JsValue {
    let _ = s;
    JsValue::NULL
}

#[wasm_bindgen(getter_with_clone)]
pub struct OnMovieLoadedCallbackData {
    pub version: u16,
    pub test_val: String,
}

#[wasm_bindgen(getter_with_clone)]
pub struct OnScriptErrorCallbackData {
    pub message: String,
    pub script_member_ref: Option<JsBridgeMemberRef>,
    pub handler_name: Option<String>,
    pub is_paused: bool,
}

impl Into<js_sys::Map> for OnScriptErrorCallbackData {
    fn into(self) -> js_sys::Map {
        let map = js_sys::Map::new();
        map.str_set("message", &safe_js_string(&self.message));
        if let Some(script_member_ref) = self.script_member_ref {
            map.str_set("script_member_ref", &script_member_ref.to_js_value());
        } else {
            map.str_set("script_member_ref", &JsValue::NULL);
        }
        if let Some(handler_name) = self.handler_name {
            map.str_set("handler_name", &safe_js_string(&handler_name));
        } else {
            map.str_set("handler_name", &JsValue::NULL);
        }
        map.str_set("is_paused", &JsValue::from_bool(self.is_paused));
        map
    }
}

#[derive(Clone)]
#[wasm_bindgen(getter_with_clone)]
pub struct JsBridgeBreakpoint {
    pub script_name: String,
    pub handler_name: String,
    pub bytecode_index: usize,
}

impl Into<js_sys::Map> for JsBridgeBreakpoint {
    fn into(self) -> js_sys::Map {
        let map = js_sys::Map::new();
        map.str_set("script_name", &safe_js_string(&self.script_name));
        map.str_set("handler_name", &safe_js_string(&self.handler_name));
        map.str_set("bytecode_index", &JsValue::from(self.bytecode_index as u32));
        map
    }
}

pub type JsBridgeMemberRef = Vec<i32>;
pub type JsBridgeDatum = js_sys::Object;

pub struct JsBridgeScope {
    pub script_member_ref: JsBridgeMemberRef,
    /// Name of the script the frame is running, resolved here so the call
    /// stack can say which script a handler belongs to without the debug UI
    /// having to hold every cast's member list.
    pub script_member_name: String,
    pub bytecode_index: u32,
    pub handler_name: String,
    pub locals: HashMap<String, DatumRef>,
    pub stack: Vec<DatumRef>,
    pub args: Vec<DatumRef>,
    /// Parameter names, positionally matching `args`. Kept alongside the values
    /// rather than folded into a map because argument order is meaningful, and
    /// resolved here for the same reason locals are: the ids only mean anything
    /// against the owning cast's name table.
    pub arg_names: Vec<String>,
}

impl Into<js_sys::Map> for JsBridgeScope {
    fn into(self) -> js_sys::Map {
        let map = js_sys::Map::new();
        map.str_set("script_member_ref", &self.script_member_ref.to_js_value());
        map.str_set("script_member_name", &safe_js_string(&self.script_member_name));
        map.str_set("bytecode_index", &JsValue::from(self.bytecode_index));
        map.str_set("handler_name", &safe_js_string(&self.handler_name));

        let locals = js_sys::Map::new();
        for (k, v) in self.locals {
            locals.set(&safe_js_string(&k), &v.unwrap().to_js_value());
        }
        map.str_set("locals", &locals.to_js_object());

        let stack = js_sys::Array::new();
        for item in self.stack {
            stack.push(&item.unwrap().to_js_value());
        }
        map.str_set("stack", &stack);

        let args = js_sys::Array::new();
        for item in self.args {
            args.push(&item.unwrap().to_js_value());
        }
        map.str_set("args", &args);

        let arg_names = js_sys::Array::new();
        for name in self.arg_names {
            arg_names.push(&safe_js_string(&name));
        }
        map.str_set("arg_names", &arg_names);

        map
    }
}

impl ToJsValue for Vec<i32> {
    fn to_js_value(&self) -> JsValue {
        let array = js_sys::Array::new();
        for item in self {
            array.push(&JsValue::from_f64(*item as f64));
        }
        array.into()
    }
}

impl ToJsValue for Vec<u32> {
    fn to_js_value(&self) -> JsValue {
        let array = js_sys::Array::new();
        for item in self {
            array.push(&JsValue::from_f64(*item as f64));
        }
        array.into()
    }
}

impl ToJsValue for Vec<usize> {
    fn to_js_value(&self) -> JsValue {
        let array = js_sys::Array::new();
        for item in self {
            array.push(&JsValue::from_f64(*item as f64));
        }
        array.into()
    }
}

impl CastMemberRef {
    pub fn to_js(&self) -> JsBridgeMemberRef {
        vec![self.cast_lib, self.cast_member]
    }
}

// Coalescing flags for the two dispatches heavy enough to matter. Both are
// called repeatedly while a movie loads (once per member added, once per score
// mutation); without this each call would serialize the whole collection.
// wasm is single-threaded, so thread_local is just "global" here.
thread_local! {
    static SCORE_DIRTY: std::cell::Cell<bool> = std::cell::Cell::new(false);
    static PENDING_CAST_LISTS: std::cell::RefCell<std::collections::HashSet<u32>> =
        std::cell::RefCell::new(std::collections::HashSet::new());
}

#[wasm_bindgen(module = "dirplayer-js-api")]
extern "C" {
    pub fn onMovieLoaded(test: OnMovieLoadedCallbackData);
    pub fn onMovieLoadFailed(path: &str, error: &str);
    pub fn onCastListChanged(names: Array);
    pub fn onCastLibNameChanged(cast_number: u32, name: &str);
    #[wasm_bindgen(catch)]
    pub fn onCastMemberListChanged(cast_number: u32, members: js_sys::Object) -> Result<(), JsValue>;
    #[wasm_bindgen(catch)]
    pub fn onCastMemberChanged(member_ref: JsValue, member: js_sys::Object) -> Result<(), JsValue>;
    #[wasm_bindgen(catch)]
    pub fn onScoreChanged(snapshot: js_sys::Object, owner_key: &str) -> Result<(), JsValue>;
    #[wasm_bindgen(catch)]
    pub fn onChannelChanged(channel: i16, snapshot: js_sys::Object, owner_key: &str) -> Result<(), JsValue>;
    #[wasm_bindgen(catch)]
    pub fn onChannelDisplayNameChanged(channel: i16, display_name: &str, owner_key: &str) -> Result<(), JsValue>;
    #[wasm_bindgen(catch)]
    pub fn onChannelDisplayNamesChanged(names: js_sys::Object, owner_key: &str) -> Result<(), JsValue>;
    pub fn onFrameChanged(frame: u32);
    #[wasm_bindgen(catch)]
    pub fn onScriptError(data: js_sys::Object) -> Result<(), JsValue>;
    pub fn onScopeListChanged(scopes: Vec<js_sys::Object>);
    pub fn onBreakpointListChanged(data: Vec<js_sys::Object>);
    pub fn onGlobalListChanged(data: js_sys::Object);
    pub fn onScriptErrorCleared();
    #[wasm_bindgen(catch)]
    pub fn onDebugMessage(message: &str) -> Result<(), JsValue>;
    #[wasm_bindgen(catch)]
    pub fn onDebugMessageOwned(owner_key: &str, message: &str) -> Result<(), JsValue>;
    pub fn onDebugContent(content: js_sys::Object);
    pub fn onScheduleTimeout(timeout_name: &str, interval: u32);
    pub fn onClearTimeout(timeout_name: &str);
    pub fn onClearTimeouts();
    #[wasm_bindgen(catch)]
    pub fn onDatumSnapshot(datum_id: DatumId, data: js_sys::Object) -> Result<(), JsValue>;
    #[wasm_bindgen(catch)]
    pub fn onDatumSnapshotOwned(owner_key: &str, datum_id: DatumId, data: js_sys::Object) -> Result<(), JsValue>;
    #[wasm_bindgen(catch)]
    pub fn onScriptInstanceSnapshot(script_ref: ScriptInstanceId, data: js_sys::Object) -> Result<(), JsValue>;
    #[wasm_bindgen(catch)]
    pub fn onScriptInstanceSnapshotOwned(owner_key: &str, script_ref: ScriptInstanceId, data: js_sys::Object) -> Result<(), JsValue>;
    #[wasm_bindgen(catch)]
    pub fn onScriptErrorOwned(owner_key: &str, data: js_sys::Object) -> Result<(), JsValue>;
    pub fn onExternalEvent(event: &str);
    pub fn onFlashMemberLoaded(sprite_num: i32, cast_lib: i32, cast_member: i32, swf_data: &[u8], width: u32, height: u32, paused_at_start: bool, asserted_frame: i32, owner_key: &str);
    pub fn onFlashMemberUnloaded(sprite_num: i32, owner_key: &str);
    pub fn onFlashResetAll(owner_key: &str);
    pub fn onStageSizeChanged(width: u32, height: u32, center: bool);
}

pub struct JsApi {}

#[cfg(target_arch = "wasm32")]
impl JsApi {
    pub fn dispatch_datum_snapshot(
        datum_ref: &DatumRef,
        symbols: &SymbolTable,
        player: &DirPlayer,
    ) {
        let snapshot = datum_to_js_bridge(datum_ref, symbols, player, 0);
        onDatumSnapshot(datum_ref.unwrap(), snapshot);
    }
    pub fn dispatch_script_instance_snapshot(
        script_ref: Option<ScriptInstanceRef>,
        symbols: &SymbolTable,
        player: &DirPlayer,
    ) {
        let datum = if script_ref.is_none() {
            Datum::Void
        } else {
            Datum::ScriptInstanceRef(script_ref.clone().unwrap())
        };
        let snapshot = concrete_datum_to_js_bridge(&datum, symbols, player, 0);
        onScriptInstanceSnapshot(script_ref.map(|script_ref| *script_ref).unwrap_or(0), snapshot);
    }
    pub fn dispatch_schedule_timeout(timeout_name: &str, interval: u32) {
        onScheduleTimeout(timeout_name, interval);
    }
    pub fn dispatch_clear_timeout(timeout_name: &str) {
        onClearTimeout(timeout_name);
    }
    #[allow(dead_code)]
    pub fn dispatch_clear_timeouts() {
        onClearTimeouts();
    }
    pub fn dispatch_flash_member_loaded(sprite_num: i32, cast_lib: i32, cast_member: i32, swf_data: &[u8], width: u32, height: u32, paused_at_start: bool, asserted_frame: i32, owner_key: &str) {
        onFlashMemberLoaded(sprite_num, cast_lib, cast_member, swf_data, width, height, paused_at_start, asserted_frame, owner_key);
    }
    pub fn dispatch_flash_member_unloaded(sprite_num: i32, owner_key: &str) {
        onFlashMemberUnloaded(sprite_num, owner_key);
    }
    /// Tear down every live Flash/Ruffle instance. Called on movie reset so
    /// a previous movie's Ruffle players (their per-frame capture RAF loops
    /// and still-playing SWF audio) don't leak across a movie switch — the
    /// per-sprite unload path only fires for sprites the new frame changed.
    pub fn dispatch_flash_reset_all(owner_key: &str) {
        onFlashResetAll(owner_key);
    }
    pub fn dispatch_stage_size_changed(width: u32, height: u32, center: bool) {
        // Only the host player (id 0) owns the frontend stage. A nested `#movie`
        // sub-player renders headless into a bitmap; if it sets `the stage.rect`
        // / `drawRect` / `centerStage` it must NOT resize the real frontend stage
        // (that caused the host stage to snap to the sub's dimensions → "zoomed"
        // flicker, since the sub re-set its rect during play).
        if unsafe { crate::player::ACTIVE_PLAYER_ID } != 0 {
            return;
        }
        onStageSizeChanged(width, height, center);
    }
    pub fn dispatch_movie_loaded(dir_file: &DirectorFile) {
        let test = dir_file
            .cast_entries
            .iter()
            .map(|cast| cast.name.to_owned())
            .collect_vec()
            .join(", ");

        onMovieLoaded(OnMovieLoadedCallbackData {
            version: dir_file.version,
            test_val: test,
        });
    }

    pub fn dispatch_movie_load_failed(path: &str, error: &str) {
        onMovieLoadFailed(path, error);
    }

    /// Collects all chunk IDs that are transitive descendants of `root_id` in the KeyTable,
    /// plus root_id itself. This walks the parent→children relationship recursively:
    /// KeyTable entries map section_id (child) → cast_id (parent).
    /// Movie-wide structural chunks that belong to the movie, not to any cast.
    ///
    /// In a RIFX file the movie-root owner id (1024) is *also* the first
    /// internal cast's id, and these chunks are owned by 1024. Walking a
    /// cast's KeyTable descendants from `cast.id` therefore sweeps them up,
    /// which made `get_movie_top_level_chunks` exclude the entire movie (it
    /// left only the ownerless ILS/KEY* — SpongeBob "JellyFishin'"). Treating
    /// these fourccs as never-cast-content keeps them in the movie view and
    /// out of the cast views.
    fn is_movie_level_fourcc(fourcc: u32) -> bool {
        matches!(
            fourcc_to_string(fourcc).trim(),
            "DRCF" | "VWCF" | "MCsL" | "Sord" | "VWFI" | "VWLB" | "VWSC" | "FXmp" | "XTRl" | "ccl"
        )
    }

    fn collect_cast_descendants(
        root_id: u32,
        children_map: &HashMap<u32, Vec<u32>>,
    ) -> std::collections::HashSet<u32> {
        let mut result = std::collections::HashSet::new();
        let mut stack = vec![root_id];
        while let Some(id) = stack.pop() {
            if result.insert(id) {
                if let Some(children) = children_map.get(&id) {
                    for child in children {
                        stack.push(*child);
                    }
                }
            }
        }
        result
    }

    /// Builds a parent→children adjacency map from the KeyTable.
    fn build_children_map(dir_file: &DirectorFile) -> HashMap<u32, Vec<u32>> {
        let mut children_map: HashMap<u32, Vec<u32>> = HashMap::new();
        if let Some(kt) = dir_file.key_table.as_ref() {
            for entry in kt.entries.iter().take(kt.used_count as usize) {
                children_map.entry(entry.cast_id).or_default().push(entry.section_id);
            }
        }
        children_map
    }

    pub fn get_cast_chunk_list_for(player: &DirPlayer, cast_number: u32) -> js_sys::Object {
        let result = js_sys::Map::new();

        let cast_lib = match player.movie.cast_manager.get_cast_or_null(cast_number) {
            Some(c) => c,
            None => return result.to_js_object(),
        };

        // Find the DirectorFile that contains the chunks
        let dir_file = if cast_lib.is_external {
            player.dir_cache.get(cast_lib.file_name.as_str()).map(|file| file.as_ref())
        } else {
            player.movie.file.as_ref()
        };

        let dir_file = match dir_file {
            Some(f) => f,
            None => return result.to_js_object(),
        };

        // Find the CastDef that matches this cast
        let cast_def = if cast_lib.is_external {
            dir_file.casts.first()
        } else {
            let cast_entry = dir_file.cast_entries.get((cast_number as usize).wrapping_sub(1));
            cast_entry.and_then(|entry| {
                dir_file.casts.iter().find(|cd| cd.id == entry.id)
            })
        };

        let cast_def = match cast_def {
            Some(cd) => cd,
            None => return result.to_js_object(),
        };

        let chunk_container = &dir_file.chunk_container;
        let key_table = dir_file.key_table.as_ref();

        // Build owner_map (child → parent) from KeyTable
        let owner_map: HashMap<u32, u32> = key_table
            .map(|kt| {
                kt.entries.iter()
                    .take(kt.used_count as usize)
                    .map(|e| (e.section_id, e.cast_id))
                    .collect()
            })
            .unwrap_or_default();

        // Build parent → children map and collect ALL transitive descendants of the cast root.
        // This captures structural chunks like Lctx → Lscr, Lnam, etc.
        let children_map = Self::build_children_map(dir_file);
        let mut cast_chunk_ids = Self::collect_cast_descendants(cast_def.id, &children_map);

        // Also include all chunks from section_to_member (CASt member chunks and their media
        // children). These are referenced from the CAS* member_ids array, not through the
        // KeyTable parent-child chain, so collect_cast_descendants doesn't find them.
        for section_id in cast_def.section_to_member.keys() {
            cast_chunk_ids.insert(*section_id);
        }

        // Also include Lscr and Lnam chunks referenced internally by the Lctx chunk.
        // These are NOT in the KeyTable as children of Lctx, so neither
        // collect_cast_descendants nor section_to_member finds them.
        for section_id in &cast_def.lctx_child_section_ids {
            cast_chunk_ids.insert(*section_id);
        }

        // Drop movie-level structural chunks that got swept in via the
        // owner-id-1024 collision (see is_movie_level_fourcc) so they don't
        // show up under this cast.
        cast_chunk_ids.retain(|id| {
            chunk_container
                .chunk_info
                .get(id)
                .map(|ci| !Self::is_movie_level_fourcc(ci.fourcc))
                .unwrap_or(true)
        });

        // Emit all chunks that belong to this cast
        for chunk_id in &cast_chunk_ids {
            let chunk_info = match chunk_container.chunk_info.get(chunk_id) {
                Some(ci) => ci,
                None => continue,
            };

            let fourcc_str = fourcc_to_string(chunk_info.fourcc);
            let chunk_map = js_sys::Map::new();
            chunk_map.str_set("id", &JsValue::from_f64(*chunk_id as f64));
            chunk_map.str_set("fourcc", &safe_js_string(&fourcc_str));
            chunk_map.str_set("len", &JsValue::from_f64(chunk_info.len as f64));
            chunk_map.str_set("castLib", &JsValue::from_f64(cast_number as f64));

            if let Some(owner_id) = owner_map.get(chunk_id) {
                chunk_map.str_set("owner", &JsValue::from_f64(*owner_id as f64));
            } else if cast_def.lctx_child_section_ids.contains(chunk_id) {
                // Lscr/Lnam chunks aren't in the KeyTable, so set their owner
                // to the Lctx section_id so they appear under it in the tree view.
                if let Some(lctx_sid) = cast_def.lctx_section_id {
                    chunk_map.str_set("owner", &JsValue::from_f64(lctx_sid as f64));
                }
            }

            // Annotate with member info if this chunk belongs to a specific member
            if let Some((member_number, member_name)) = cast_def.section_to_member.get(chunk_id) {
                chunk_map.str_set("memberNumber", &JsValue::from_f64(*member_number as f64));
                chunk_map.str_set("memberName", &safe_js_string(member_name));
            }

            result.set(
                &JsValue::from_f64(*chunk_id as f64),
                &chunk_map.to_js_object(),
            );
        }

        result.to_js_object()
    }

    /// Returns all chunks from the main movie file that are NOT associated with any cast.
    pub fn get_movie_top_level_chunks(player: &DirPlayer) -> js_sys::Object {
        let result = js_sys::Map::new();

        let dir_file = match player.movie.file.as_ref() {
            Some(f) => f,
            None => return result.to_js_object(),
        };

        let chunk_container = &dir_file.chunk_container;
        let key_table = dir_file.key_table.as_ref();

        // Build parent → children map and collect ALL transitive descendants of every cast root
        let children_map = Self::build_children_map(dir_file);
        let mut cast_section_ids = std::collections::HashSet::new();
        for cast_def in &dir_file.casts {
            let descendants = Self::collect_cast_descendants(cast_def.id, &children_map);
            for id in descendants {
                cast_section_ids.insert(id);
            }
            // Also exclude chunks belonging to cast members (CASt chunks and their media
            // children). These are referenced from the CAS* member_ids array, not through
            // the KeyTable parent-child chain, so collect_cast_descendants doesn't find them.
            for section_id in cast_def.section_to_member.keys() {
                cast_section_ids.insert(*section_id);
            }
            // Also exclude Lscr and Lnam chunks referenced internally by Lctx.
            for section_id in &cast_def.lctx_child_section_ids {
                cast_section_ids.insert(*section_id);
            }
        }

        // Don't exclude movie-level structural chunks (config, score, cast
        // list, etc.). They're owned by the movie-root id 1024 which collides
        // with the first cast's id, so the descendant walk above wrongly
        // captured them — leaving the movie view with only ILS/KEY*.
        cast_section_ids.retain(|id| {
            chunk_container
                .chunk_info
                .get(id)
                .map(|ci| !Self::is_movie_level_fourcc(ci.fourcc))
                .unwrap_or(true)
        });

        // Build owner_map from KeyTable
        let owner_map: HashMap<u32, u32> = key_table
            .map(|kt| {
                kt.entries.iter()
                    .take(kt.used_count as usize)
                    .map(|e| (e.section_id, e.cast_id))
                    .collect()
            })
            .unwrap_or_default();

        for (chunk_id, chunk_info) in &chunk_container.chunk_info {
            if cast_section_ids.contains(chunk_id) {
                continue;
            }

            let fourcc_str = fourcc_to_string(chunk_info.fourcc);
            let chunk_map = js_sys::Map::new();
            chunk_map.str_set("id", &JsValue::from_f64(*chunk_id as f64));
            chunk_map.str_set("fourcc", &safe_js_string(&fourcc_str));
            chunk_map.str_set("len", &JsValue::from_f64(chunk_info.len as f64));

            if let Some(owner_id) = owner_map.get(chunk_id) {
                chunk_map.str_set("owner", &JsValue::from_f64(*owner_id as f64));
            }

            result.set(
                &JsValue::from_f64(*chunk_id as f64),
                &chunk_map.to_js_object(),
            );
        }

        result.to_js_object()
    }

    /// Returns the raw bytes of a chunk by ID from the specified cast's DirectorFile.
    /// If cast_number is 0, uses the main movie file.
    pub fn get_chunk_bytes(player: &DirPlayer, cast_number: u32, chunk_id: u32) -> Option<Vec<u8>> {
        let dir_file = if cast_number == 0 {
            player.movie.file.as_ref()
        } else {
            let cast_lib = player.movie.cast_manager.get_cast_or_null(cast_number)?;
            if cast_lib.is_external {
                player.dir_cache.get(cast_lib.file_name.as_str()).map(|file| file.as_ref())
            } else {
                player.movie.file.as_ref()
            }
        };

        let dir_file = dir_file?;
        dir_file.chunk_container.cached_chunk_views.get(&chunk_id).cloned()
    }

    fn chunk_to_js(chunk: &Chunk, symbols: &SymbolTable) -> js_sys::Object {
        let map = js_sys::Map::new();
        match chunk {
            Chunk::Cast(c) => {
                map.str_set("type", &JsValue::from_str("CAS*"));
                let ids = js_sys::Array::new();
                for id in &c.member_ids {
                    ids.push(&JsValue::from_f64(*id as f64));
                }
                map.str_set("member_ids", &ids);
            }
            Chunk::CastMember(c) => {
                map.str_set("type", &JsValue::from_str("CASt"));
                map.str_set("member_type", &JsValue::from_str(&format!("{:?}", c.member_type)));
                if let Some(info) = &c.member_info {
                    map.str_set("name", &JsValue::from_str(&ascii_safe(&info.name)));
                    if !info.script_src_text.is_empty() {
                        map.str_set("script_src_text", &JsValue::from_str(&ascii_safe(&info.script_src_text)));
                    }
                    map.str_set("script_id", &JsValue::from_f64(info.header.script_id as f64));
                    map.str_set("flags", &JsValue::from_f64(info.header.flags as f64));
                }
                // Serialize type-specific data
                match &c.specific_data {
                    crate::director::chunks::cast_member::CastMemberSpecificData::Script(st) => {
                        map.str_set("script_type", &JsValue::from_str(&format!("{:?}", st)));
                    }
                    crate::director::chunks::cast_member::CastMemberSpecificData::Bitmap(bi) => {
                        let bm = js_sys::Map::new();
                        bm.str_set("width", &JsValue::from_f64(bi.width as f64));
                        bm.str_set("height", &JsValue::from_f64(bi.height as f64));
                        bm.str_set("reg_x", &JsValue::from_f64(bi.reg_x as f64));
                        bm.str_set("reg_y", &JsValue::from_f64(bi.reg_y as f64));
                        bm.str_set("bit_depth", &JsValue::from_f64(bi.bit_depth as f64));
                        bm.str_set("palette_id", &JsValue::from_f64(bi.palette_id as f64));
                        map.str_set("bitmap_info", &bm.to_js_object());
                    }
                    crate::director::chunks::cast_member::CastMemberSpecificData::Text(ti) => {
                        let tm = js_sys::Map::new();
                        tm.str_set("width", &JsValue::from_f64(ti.width as f64));
                        tm.str_set("height", &JsValue::from_f64(ti.height as f64));
                        tm.str_set("editable", &JsValue::from_bool(ti.editable));
                        tm.str_set("box_type", &JsValue::from_f64(ti.box_type as f64));
                        tm.str_set("anti_alias", &JsValue::from_bool(ti.anti_alias));
                        map.str_set("text_info", &tm.to_js_object());
                    }
                    crate::director::chunks::cast_member::CastMemberSpecificData::Field(fi) => {
                        let fm = js_sys::Map::new();
                        fm.str_set("alignment", &JsValue::from_f64(fi.alignment as f64));
                        map.str_set("field_info", &fm.to_js_object());
                    }
                    _ => {}
                }
            }
            Chunk::CastList(c) => {
                map.str_set("type", &JsValue::from_str("MCsL"));
                let entries = js_sys::Array::new();
                for entry in &c.entries {
                    let em = js_sys::Map::new();
                    em.str_set("name", &JsValue::from_str(&ascii_safe(&entry.name)));
                    em.str_set("file_path", &JsValue::from_str(&ascii_safe(&entry.file_path)));
                    em.str_set("id", &JsValue::from_f64(entry.id as f64));
                    em.str_set("min_member", &JsValue::from_f64(entry.min_member as f64));
                    em.str_set("max_member", &JsValue::from_f64(entry.max_member as f64));
                    em.str_set("preload_settings", &JsValue::from_f64(entry.preload_settings as f64));
                    entries.push(&em.to_js_object());
                }
                map.str_set("entries", &entries);
            }
            Chunk::KeyTable(kt) => {
                map.str_set("type", &JsValue::from_str("KEY*"));
                map.str_set("used_count", &JsValue::from_f64(kt.used_count as f64));
                map.str_set("entry_count", &JsValue::from_f64(kt.entry_count as f64));
                let entries = js_sys::Array::new();
                for entry in kt.entries.iter().take(kt.used_count as usize) {
                    let em = js_sys::Map::new();
                    em.str_set("section_id", &JsValue::from_f64(entry.section_id as f64));
                    em.str_set("cast_id", &JsValue::from_f64(entry.cast_id as f64));
                    em.str_set("fourcc", &JsValue::from_str(&fourcc_to_string(entry.fourcc)));
                    entries.push(&em.to_js_object());
                }
                map.str_set("entries", &entries);
            }
            Chunk::ScriptContext(sc) => {
                map.str_set("type", &JsValue::from_str("Lctx"));
                map.str_set("entry_count", &JsValue::from_f64(sc.entry_count as f64));
                map.str_set("lnam_section_id", &JsValue::from_f64(sc.lnam_section_id as f64));
                let entries = js_sys::Array::new();
                for entry in &sc.section_map {
                    let em = js_sys::Map::new();
                    em.str_set("section_id", &JsValue::from_f64(entry.section_id as f64));
                    entries.push(&em.to_js_object());
                }
                map.str_set("section_map", &entries);
            }
            Chunk::ScriptNames(sn) => {
                map.str_set("type", &JsValue::from_str("Lnam"));
                let names = js_sys::Array::new();
                for name in &sn.names {
                    names.push(&JsValue::from_str(&ascii_safe(name)));
                }
                map.str_set("names", &names);
            }
            Chunk::Script(sc) => {
                map.str_set("type", &JsValue::from_str("Lscr"));
                map.str_set("handler_count", &JsValue::from_f64(sc.handlers.len() as f64));
                map.str_set("literal_count", &JsValue::from_f64(sc.literals.len() as f64));
                let prop_ids = js_sys::Array::new();
                for id in &sc.property_name_ids {
                    prop_ids.push(&JsValue::from_f64(*id as f64));
                }
                map.str_set("property_name_ids", &prop_ids);
                let handlers = js_sys::Array::new();
                for handler in &sc.handlers {
                    let hm = js_sys::Map::new();
                    hm.str_set("name_id", &JsValue::from_f64(handler.name_id as f64));
                    hm.str_set("bytecode_count", &JsValue::from_f64(handler.bytecode_array.len() as f64));
                    let arg_ids = js_sys::Array::new();
                    for id in &handler.argument_name_ids {
                        arg_ids.push(&JsValue::from_f64(*id as f64));
                    }
                    hm.str_set("argument_name_ids", &arg_ids);
                    let local_ids = js_sys::Array::new();
                    for id in &handler.local_name_ids {
                        local_ids.push(&JsValue::from_f64(*id as f64));
                    }
                    hm.str_set("local_name_ids", &local_ids);
                    let global_ids = js_sys::Array::new();
                    for id in &handler.global_name_ids {
                        global_ids.push(&JsValue::from_f64(*id as f64));
                    }
                    hm.str_set("global_name_ids", &global_ids);
                    handlers.push(&hm.to_js_object());
                }
                map.str_set("handlers", &handlers);
                let literals = js_sys::Array::new();
                for literal in &sc.literals {
                    let lm = js_sys::Map::new();
                    lm.str_set("type", &JsValue::from_str(&literal.type_str()));
                    match literal {
                        Datum::Int(v) => {
                            lm.str_set("value", &JsValue::from_f64(*v as f64));
                        }
                        Datum::Float(v) => {
                            lm.str_set("value", &JsValue::from_f64(*v));
                        }
                        Datum::String(s) => {
                            lm.str_set("value", &JsValue::from_str(&ascii_safe(&s)));
                        }
                        Datum::Symbol(s) => {
                            let value = symbols
                                .display(s)
                                .map(ascii_safe)
                                .unwrap_or_else(|_| "<foreign-symbol>".to_owned());
                            lm.str_set("value", &JsValue::from_str(&value));
                        }
                        Datum::JavaScript(data) => {
                            lm.str_set("size", &JsValue::from_f64(data.len() as f64));
                            lm.str_set("bytes", &js_sys::Uint8Array::from(&data[..]));
                        }
                        Datum::Void => {}
                        _ => {
                            lm.str_set("value", &JsValue::from_str(&literal.type_str()));
                        }
                    }
                    literals.push(&lm.to_js_object());
                }
                map.str_set("literals", &literals);
            }
            Chunk::Config(c) => {
                map.str_set("type", &JsValue::from_str("VWCF"));
                map.str_set("director_version", &JsValue::from_f64(c.director_version as f64));
                map.str_set("movie_top", &JsValue::from_f64(c.movie_top as f64));
                map.str_set("movie_left", &JsValue::from_f64(c.movie_left as f64));
                map.str_set("movie_bottom", &JsValue::from_f64(c.movie_bottom as f64));
                map.str_set("movie_right", &JsValue::from_f64(c.movie_right as f64));
                map.str_set("min_member", &JsValue::from_f64(c.min_member as f64));
                map.str_set("max_member", &JsValue::from_f64(c.max_member as f64));
                map.str_set("frame_rate", &JsValue::from_f64(c.frame_rate as f64));
                map.str_set("bit_depth", &JsValue::from_f64(c.bit_depth as f64));
                map.str_set("platform", &JsValue::from_f64(c.platform as f64));
            }
            Chunk::Text(t) => {
                map.str_set("type", &JsValue::from_str("STXT"));
                map.str_set("text", &JsValue::from_str(&ascii_safe(&t.text)));
                map.str_set("text_length", &JsValue::from_f64(t.text_length as f64));
                let runs = t.parse_formatting_runs();
                let runs_arr = js_sys::Array::new();
                for run in &runs {
                    let rm = js_sys::Map::new();
                    rm.str_set("start_position", &JsValue::from_f64(run.start_position as f64));
                    rm.str_set("font_id", &JsValue::from_f64(run.font_id as f64));
                    rm.str_set("font_size", &JsValue::from_f64(run.font_size as f64));
                    rm.str_set("style", &JsValue::from_f64(run.style as f64));
                    runs_arr.push(&rm.to_js_object());
                }
                map.str_set("formatting_runs", &runs_arr);
            }
            Chunk::Palette(p) => {
                map.str_set("type", &JsValue::from_str("CLUT"));
                let colors = js_sys::Array::new();
                for (r, g, b) in &p.colors {
                    colors.push(&JsValue::from_str(&format!("#{:02x}{:02x}{:02x}", r, g, b)));
                }
                map.str_set("colors", &colors);
            }
            Chunk::Sound(s) => {
                map.str_set("type", &JsValue::from_str("snd "));
                map.str_set("channels", &JsValue::from_f64(s.channels() as f64));
                map.str_set("sample_rate", &JsValue::from_f64(s.sample_rate() as f64));
                map.str_set("bits_per_sample", &JsValue::from_f64(s.bits_per_sample() as f64));
                map.str_set("sample_count", &JsValue::from_f64(s.sample_count() as f64));
                map.str_set("codec", &JsValue::from_str(&ascii_safe(&s.codec())));
                map.str_set("data_size", &JsValue::from_f64(s.data().len() as f64));
            }
            Chunk::Score(sc) => {
                map.str_set("type", &JsValue::from_str("VWSC"));
                map.str_set("entry_count", &JsValue::from_f64(sc.header.entry_count as f64));
                map.str_set("frame_interval_count", &JsValue::from_f64(sc.frame_intervals.len() as f64));
            }
            Chunk::FrameLabels(fl) => {
                map.str_set("type", &JsValue::from_str("VWLB"));
                let labels = js_sys::Array::new();
                for label in &fl.labels {
                    let lm = js_sys::Map::new();
                    lm.str_set("frame_num", &JsValue::from_f64(label.frame_num as f64));
                    lm.str_set("label", &JsValue::from_str(&ascii_safe(&label.label)));
                    labels.push(&lm.to_js_object());
                }
                map.str_set("labels", &labels);
            }
            Chunk::Bitmap(b) => {
                map.str_set("type", &JsValue::from_str("BITD"));
                map.str_set("data_size", &JsValue::from_f64(b.data.len() as f64));
                map.str_set("version", &JsValue::from_f64(b.version as f64));
            }
            Chunk::XMedia(xm) => {
                map.str_set("type", &JsValue::from_str("XMED"));
                map.str_set("data_size", &JsValue::from_f64(xm.raw_data.len() as f64));
                if xm.is_pfr_font() {
                    map.str_set("content_type", &JsValue::from_str("PFR1 Font"));
                    if let Some(font) = xm.parse_pfr_font() {
                        map.str_set("font_name", &JsValue::from_str(&ascii_safe(&font.font_name)));
                        map.str_set("outline_glyph_count", &JsValue::from_f64(font.parsed.glyphs.len() as f64));
                        map.str_set("bitmap_glyph_count", &JsValue::from_f64(font.parsed.bitmap_glyphs.len() as f64));
                        map.str_set("target_em_px", &JsValue::from_f64(font.parsed.target_em_px as f64));
                    }
                } else if xm.is_styled_text() {
                    map.str_set("content_type", &JsValue::from_str("Styled Text"));
                    if let Some(st) = xm.parse_styled_text() {
                        map.str_set("text", &JsValue::from_str(&ascii_safe(&st.text)));
                        map.str_set("alignment", &JsValue::from_str(&format!("{:?}", st.alignment)));
                        map.str_set("word_wrap", &JsValue::from_bool(st.word_wrap));
                        map.str_set("width", &JsValue::from_f64(st.width as f64));
                        map.str_set("height", &JsValue::from_f64(st.height as f64));
                        map.str_set("line_count", &JsValue::from_f64(st.line_count as f64));
                        map.str_set("fixed_line_space", &JsValue::from_f64(st.fixed_line_space as f64));
                        let spans = js_sys::Array::new();
                        for span in &st.styled_spans {
                            let sm = js_sys::Map::new();
                            sm.str_set("text", &JsValue::from_str(&ascii_safe(&span.text)));
                            if let Some(face) = &span.style.font_face {
                                sm.str_set("font_face", &JsValue::from_str(&ascii_safe(face)));
                            }
                            if let Some(size) = span.style.font_size {
                                sm.str_set("font_size", &JsValue::from_f64(size as f64));
                            }
                            sm.str_set("bold", &JsValue::from_bool(span.style.bold));
                            sm.str_set("italic", &JsValue::from_bool(span.style.italic));
                            sm.str_set("underline", &JsValue::from_bool(span.style.underline));
                            if let Some(color) = span.style.color {
                                sm.str_set("color", &JsValue::from_str(&format!("#{:06x}", color)));
                            }
                            spans.push(&sm.to_js_object());
                        }
                        map.str_set("styled_spans", &spans);
                    }
                } else {
                    map.str_set("content_type", &JsValue::from_str("Unknown"));
                }
            }
            _ => {
                map.str_set("type", &JsValue::from_str("unsupported"));
            }
        }
        map.to_js_object()
    }

    /// Returns parsed chunk data as a JS object. Re-parses from cached raw bytes on demand.
    pub fn get_parsed_chunk(
        player: &DirPlayer,
        symbols: &SymbolTable,
        cast_number: u32,
        chunk_id: u32,
    ) -> js_sys::Object {
        let error_result = |msg: &str| -> js_sys::Object {
            let map = js_sys::Map::new();
            map.str_set("error", &JsValue::from_str(&ascii_safe(msg)));
            map.to_js_object()
        };

        let dir_file = if cast_number == 0 {
            player.movie.file.as_ref()
        } else {
            let cast_lib = match player.movie.cast_manager.get_cast_or_null(cast_number) {
                Some(c) => c,
                None => return error_result("Cast not found"),
            };
            if cast_lib.is_external {
                player.dir_cache.get(cast_lib.file_name.as_str()).map(|file| file.as_ref())
            } else {
                player.movie.file.as_ref()
            }
        };

        let dir_file = match dir_file {
            Some(f) => f,
            None => return error_result("DirectorFile not found"),
        };

        let chunk_info = match dir_file.chunk_container.chunk_info.get(&chunk_id) {
            Some(ci) => ci,
            None => return error_result("Chunk info not found"),
        };

        let raw_bytes = match dir_file.chunk_container.cached_chunk_views.get(&chunk_id) {
            Some(b) => b,
            None => return error_result("Raw bytes not found"),
        };

        // Determine lctx_capital_x for this chunk's cast
        let lctx_capital_x = dir_file.casts.iter().find(|cd| {
            cd.lctx_child_section_ids.contains(&chunk_id)
        }).map(|cd| cd.capital_x).unwrap_or(false);

        let mut rifx = RIFXReaderContext {
            after_burned: dir_file.after_burned,
            ils_body_offset: 0,
            dir_version: dir_file.version,
            lctx_capital_x,
        };

        match chunks::make_chunk(dir_file.endian, &mut rifx, chunk_info.fourcc, raw_bytes) {
            Ok(chunk) => Self::chunk_to_js(&chunk, symbols),
            Err(e) => error_result(&e),
        }
    }

    pub fn dispatch_cast_name_changed(cast_number: u32) {
        crate::player::spawn_player_local(async move {
            // Deferred task — the player may be gone by the time it runs.
            if unsafe { PLAYER_OPT.is_none() } { return; }
            let player = unsafe { crate::player::player_ref() };
            let cast = player.movie.cast_manager.get_cast(cast_number).unwrap();
            onCastLibNameChanged(cast_number, &cast.name);
        });
    }

    pub fn dispatch_cast_list_changed() {
        crate::player::spawn_player_local(async move {
            // Deferred task — the player may be gone by the time it runs.
            if unsafe { PLAYER_OPT.is_none() } { return; }
            let player = unsafe { crate::player::player_ref() };
            let names = player
                .movie
                .cast_manager
                .casts
                .iter()
                .map(|x| x.name.to_owned())
                .collect_vec();

            onCastListChanged(
                names
                    .into_iter()
                    .map(|x| safe_js_string(&x))
                    .collect::<Array>(),
            );
        });
    }

    pub fn dispatch_cast_member_list_changed(cast_number: u32) {
        // Nobody is showing this cast's members: serializing them would be
        // thousands of objects thrown away. Loading a movie fires this once per
        // member added, so the saving compounds.
        if !PENDING_CAST_LISTS.with(|pending| pending.borrow_mut().insert(cast_number)) {
            // Already queued for this tick; the flush below sends current state.
            return;
        }
        crate::player::spawn_player_local(async move {
            PENDING_CAST_LISTS.with(|pending| pending.borrow_mut().remove(&cast_number));
            // Deferred task — the player may be gone by the time it runs.
            if unsafe { PLAYER_OPT.is_none() } { return; }
            let player = unsafe { crate::player::player_ref() };
            if !player.subscribed_cast_member_lists.contains(&cast_number) { return; }
            let cast = match player.movie.cast_manager.get_cast(cast_number) {
                Ok(cast) => cast,
                Err(_) => return,
            };
            let members_iter = cast.members.values().into_iter();

            let member_list = js_sys::Map::new();
            for member in members_iter {
                let member_map = Self::get_mini_member_snapshot(member);
                member_list.set(&JsValue::from(member.number), &member_map.to_js_object());
            }

            onCastMemberListChanged(cast_number, member_list.to_js_object());
        });
    }

    pub fn dispatch_cast_member_changed(
        member_ref: CastMemberRef,
        symbols: &SymbolTable,
        player: &DirPlayer,
    ) {
        let member_map = {
            if !player.subscribed_member_refs.contains(&member_ref) {
                return;
            }
            let Ok(cast) = player.movie.cast_manager.get_cast(member_ref.cast_lib as u32) else {
                return;
            };
            let Some(member) = cast.members.get(&(member_ref.cast_member as u32)) else {
                return;
            };
            Self::get_member_snapshot(
                member,
                member_ref.cast_lib as u32,
                cast.lctx.as_ref(),
                symbols,
                player,
            )
        };
        let owner = player.owner.clone();
        async_std::task::spawn_local(async move {
            // The snapshot and initial subscription decision are owned by
            // this notification. The owner token is the only liveness check
            // needed after the borrow ends; a replacement arena cannot reuse
            // this notification's capability.
            if !owner.is_arena_live() {
                return;
            }
            onCastMemberChanged(member_ref.to_js().to_js_value(), member_map.to_js_object());
        });
    }

    pub fn on_cast_member_name_changed(slot_number: u32) {
        crate::player::spawn_player_local(async move {
            // Deferred task — the player may be gone by the time it runs.
            if unsafe { PLAYER_OPT.is_none() } { return; }
            let player = unsafe { crate::player::player_ref() };

            if player.is_subscribed_to_channel_names {
                for channel in player.movie.score.channels.iter() {
                    if channel.sprite.member.as_ref().map(|x| {
                        CastMemberRefHandlers::get_cast_slot_number(
                            x.cast_lib as u32,
                            x.cast_member as u32,
                        )
                    }) == Some(slot_number)
                    {
                        Self::dispatch_channel_name_changed(channel.number as i16);
                    }
                }
            }
        });
    }

    pub fn on_sprite_member_changed(sprite_num: i16) {
        Self::dispatch_channel_name_changed(sprite_num)
    }

    pub fn dispatch_score_changed() {
        // Coalesced: loading a movie calls this dozens of times as the score is
        // assembled, and each call would serialize the whole thing. Only the
        // first schedules a task; it sends whatever the final state is.
        if SCORE_DIRTY.with(|dirty| dirty.replace(true)) {
            return;
        }
        crate::player::spawn_player_local(async move {
            SCORE_DIRTY.with(|dirty| dirty.set(false));
            // Deferred task — the player may be gone by the time it runs.
            if unsafe { PLAYER_OPT.is_none() } { return; }
            let player = unsafe { crate::player::player_ref() };
            // Only when a score inspector is open. See is_subscribed_to_score.
            if !player.is_subscribed_to_score { return; }

            let snapshot = Self::get_score_snapshot(player, &player.movie.score);
            let owner_key = owner_key_string(&player.owner);
            onScoreChanged(snapshot.to_js_object(), &owner_key);
        });
    }

    pub fn score_snapshot_for_player(
        player: &DirPlayer,
    ) -> Option<(js_sys::Object, String)> {
        player.is_subscribed_to_score.then(|| {
            (
                Self::get_score_snapshot(player, &player.movie.score).to_js_object(),
                owner_key_string(&player.owner),
            )
        })
    }

    /// Drain owner-tagged player notifications at a host boundary. The
    /// session is borrowed only to take one event and build its JS payload;
    /// every callback runs after that borrow has been dropped. Reset or
    /// replacement during a callback invalidates the remaining batch.
    pub(crate) fn dispatch_player_notifications(
        session: RuntimeSessionHandle,
        player_id: PlayerId,
    ) -> Result<(), JsValue> {
        if session.borrow().host_event_lifecycle_pending(player_id) {
            return Ok(());
        }
        enum PreparedNotification {
            Score(js_sys::Object, String),
            Channel(i16, js_sys::Object, String),
            ChannelName(i16, String, String),
            ChannelNames(Vec<(i16, String)>, String),
            ChannelNamesSnapshot(js_sys::Object, String),
            CastMemberChanged(CastMemberRef, js_sys::Object),
            CastMemberListChanged(u32, js_sys::Object),
            DatumSnapshot(DatumId, js_sys::Object, String),
            ScriptInstanceSnapshot(ScriptInstanceId, js_sys::Object, String),
            Host(HostEvent, js_sys::Object, String),
            Backpressure(usize),
        }

        let Some((drain_owner, batch)) = session
            .borrow_mut()
            .begin_player_notification_drain(player_id)
        else {
            return Ok(());
        };
        let mut dispatch_error = None;
        let mut retry_notifications = Vec::new();
        let mut retry_host_events = Vec::new();
        let mut terminal_backpressure = false;
        let mut notifications = batch.into_iter();
        while let Some(notification) = notifications.next() {
            if session.borrow().host_event_lifecycle_pending(player_id) {
                // A notification callback may have rebound/reset the host
                // sink. Preserve this item and the remaining batch until the
                // replacement lifecycle has delivered its OwnerBound event.
                retry_notifications.push(notification);
                break;
            }
            let prepared = {
                let mut runtime = session.borrow_mut();
                if !notification.owner.same_identity(&drain_owner)
                    || !runtime.player_owner_matches(player_id, &notification.owner)
                {
                    None
                } else {
                    let owner = notification.owner.clone();
                    runtime
                        .with_player(player_id, |context| {
                            let owner_key = owner_key_string(&context.player.owner);
                            match notification.kind {
                                PlayerNotificationKind::ScoreChanged =>
                                    Self::score_snapshot_for_player(context.player)
                                        .map(|(snapshot, _)| PreparedNotification::Score(snapshot, owner_key)),
                                PlayerNotificationKind::ChannelChanged(channel) => Some(
                                    PreparedNotification::Channel(
                                        channel,
                                        Self::channel_snapshot_for_player(context.player, channel).0,
                                        owner_key,
                                    ),
                                ),
                                PlayerNotificationKind::ChannelNameChanged(channel) =>
                                    Self::channel_name_snapshot_for_player(context.player, channel)
                                        .map(|(name, _)| PreparedNotification::ChannelName(channel, name, owner_key)),
                                PlayerNotificationKind::CastMemberNameChanged(slot) => Some(
                                    PreparedNotification::ChannelNames(
                                        Self::channel_name_snapshots_for_member_slot(context.player, slot),
                                        owner_key,
                                    ),
                                ),
                                PlayerNotificationKind::ChannelNamesChanged => {
                                    if !context.player.is_subscribed_to_channel_names {
                                        return None;
                                    }
                                    let (names, owner_key) =
                                        Self::channel_names_snapshot_for_player(context.player);
                                    Some(PreparedNotification::ChannelNamesSnapshot(names, owner_key))
                                }
                                PlayerNotificationKind::CastMemberChanged(member_ref) => {
                                    if !context
                                        .player
                                        .subscribed_member_refs
                                        .contains(&member_ref)
                                    {
                                        return None;
                                    }
                                    let cast = context
                                        .player
                                        .movie
                                        .cast_manager
                                        .get_cast(member_ref.cast_lib as u32)
                                        .ok();
                                    cast.and_then(|cast| {
                                        cast.members
                                            .get(&(member_ref.cast_member as u32))
                                            .map(|member| {
                                                PreparedNotification::CastMemberChanged(
                                                    member_ref.clone(),
                                                    Self::get_member_snapshot(
                                                        member,
                                                        member_ref.cast_lib as u32,
                                                        cast.lctx.as_ref(),
                                                        context.symbols,
                                                        context.player,
                                                    )
                                                    .to_js_object(),
                                                )
                                            })
                                    })
                                }
                                PlayerNotificationKind::CastMemberListChanged(cast_number) => {
                                    if !context
                                        .player
                                        .subscribed_cast_member_lists
                                        .contains(&cast_number)
                                    {
                                        return None;
                                    }
                                    context
                                        .player
                                        .movie
                                        .cast_manager
                                        .get_cast(cast_number)
                                        .ok()
                                        .map(|cast| {
                                            let member_list = js_sys::Map::new();
                                            for member in cast.members.values() {
                                                member_list.set(
                                                    &JsValue::from(member.number),
                                                    &Self::get_mini_member_snapshot(member)
                                                        .to_js_object(),
                                                );
                                            }
                                            PreparedNotification::CastMemberListChanged(
                                                cast_number,
                                                member_list.to_js_object(),
                                            )
                                        })
                                }
                                PlayerNotificationKind::DatumSnapshot(datum_ref) => Some(
                                        PreparedNotification::DatumSnapshot(
                                            datum_ref.unwrap(),
                                        datum_to_js_bridge(
                                            &datum_ref,
                                            context.symbols,
                                            context.player,
                                            0,
                                            ),
                                            owner_key,
                                    ),
                                ),
                                PlayerNotificationKind::ScriptInstanceSnapshot(instance_id) => {
                                    let script_id = instance_id.as_ref().map(|instance| instance.id());
                                    let datum = instance_id
                                        .map(Datum::ScriptInstanceRef)
                                        .unwrap_or(Datum::Void);
                                    Some(PreparedNotification::ScriptInstanceSnapshot(
                                        script_id.unwrap_or(0),
                                        concrete_datum_to_js_bridge(
                                            &datum,
                                            context.symbols,
                                            context.player,
                                            0,
                                        ),
                                        owner_key,
                                    ))
                                }
                                PlayerNotificationKind::Host(event) => Some(
                                    PreparedNotification::Host(
                                        event.clone(),
                                        Self::host_event_to_js(&event),
                                        owner_key,
                                    ),
                                ),
                                PlayerNotificationKind::HostBackpressure(capacity) => {
                                    Some(PreparedNotification::Backpressure(capacity))
                                }
                            }
                        })
                        .flatten()
                        .map(|payload| (owner, payload))
                }
            };

            let Some((owner, payload)) = prepared else {
                continue;
            };
            if !session.borrow().player_owner_matches(player_id, &owner) {
                continue;
            }
            let result = match payload {
                PreparedNotification::Score(snapshot, owner_key) => {
                    Self::dispatch_score_snapshot(snapshot, &owner_key)
                }
                PreparedNotification::Channel(channel, snapshot, owner_key) => {
                    Self::dispatch_channel_snapshot(channel, snapshot, &owner_key)
                }
                PreparedNotification::ChannelName(channel, name, owner_key) => {
                    Self::dispatch_channel_name_snapshot(channel, &name, &owner_key)
                }
                PreparedNotification::ChannelNames(names, owner_key) => {
                    for (channel, name) in names {
                        if !session.borrow().player_owner_matches(player_id, &owner) {
                            break;
                        }
                        if let Err(error) = Self::dispatch_channel_name_snapshot(
                            channel,
                            &name,
                            &owner_key,
                        ) {
                            log::error!("owner channel-name callback failed: {:?}", error);
                            dispatch_error = Some(error);
                            break;
                        }
                    }
                    Ok(())
                }
                PreparedNotification::ChannelNamesSnapshot(names, owner_key) => {
                    Self::dispatch_channel_names_snapshot(names, &owner_key)
                }
                PreparedNotification::CastMemberChanged(member_ref, member) => {
                    onCastMemberChanged(member_ref.to_js().to_js_value(), member)
                }
                PreparedNotification::CastMemberListChanged(cast_number, members) => {
                    onCastMemberListChanged(cast_number, members)
                }
                PreparedNotification::DatumSnapshot(datum_id, snapshot, owner_key) => {
                    onDatumSnapshotOwned(&owner_key, datum_id, snapshot)
                }
                PreparedNotification::ScriptInstanceSnapshot(instance_id, snapshot, owner_key) => {
                    onScriptInstanceSnapshotOwned(&owner_key, instance_id, snapshot)
                }
                PreparedNotification::Host(host_event, event, owner_key) => {
                    let sink = { session.borrow().host_sink(player_id, &owner) };
                    match sink {
                        Some(sink) => sink.dispatch(event, &owner_key),
                        None => {
                            retry_host_events.push((owner.clone(), host_event));
                            Err(JsValue::from_str("browser host event sink is not bound"))
                        }
                    }
                }
                PreparedNotification::Backpressure(capacity) => {
                    terminal_backpressure = true;
                    Err(JsValue::from_str(&format!(
                        "owner host event mailbox overflow (capacity {})",
                        capacity
                    )))
                }
            };
            if dispatch_error.is_some() {
                break;
            }
            if let Err(error) = result {
                log::error!("owner notification callback failed: {:?}", error);
                dispatch_error = Some(error);
                break;
            }
        }
        // Preserve only unattempted host events. A callback that already
        // threw is not replayed, while an event blocked by a cleared sink is
        // restored in its original order for the next binding.
        for notification in notifications {
            let owner = notification.owner;
            match notification.kind {
                PlayerNotificationKind::Host(event) => {
                    retry_host_events.push((owner, event));
                }
                PlayerNotificationKind::HostBackpressure(_) => {}
                kind => retry_notifications.push(PlayerNotification { owner, kind }),
            }
        }
        if !retry_notifications.is_empty() {
            let pending = std::mem::take(&mut retry_notifications);
            let restore_result = session.borrow_mut().with_player(player_id, |context| {
                if context.player.owner.same_identity(&drain_owner) {
                    context.player.prepend_player_notifications(pending);
                }
                Ok::<(), JsValue>(())
            });
            if let Some(Err(error)) = restore_result {
                dispatch_error.get_or_insert(error);
            }
        }
        if !retry_host_events.is_empty() {
            let pending = retry_host_events
                .into_iter()
                .map(|(_, event)| event)
                .collect();
            let restore_result = session.borrow_mut().with_player(player_id, |context| {
                if context.player.owner.same_identity(&drain_owner) {
                    context.player.prepend_host_events(pending).map_err(|error| {
                        JsValue::from_str(&format!("host event mailbox overflow: {error:?}"))
                    })?;
                }
                Ok(())
            });
            if let Some(Err(error)) = restore_result {
                dispatch_error.get_or_insert(error);
                terminal_backpressure = true;
            }
        }
        if terminal_backpressure {
            session
                .borrow_mut()
                .cancel_host_backpressured_owner(player_id, &drain_owner);
        }
        session
            .borrow_mut()
            .finish_player_notification_drain(player_id, &drain_owner);
        dispatch_error.map_or(Ok(()), Err)
    }

    pub(crate) fn host_event_to_js(event: &HostEvent) -> js_sys::Object {
        let map = js_sys::Map::new();
        match event {
            HostEvent::OwnerBound { owner_key } => {
                map.str_set("type", &safe_js_string("ownerBound"));
                map.str_set("ownerKey", &safe_js_string(owner_key));
            }
            HostEvent::OwnerRetired { owner_key } => {
                map.str_set("type", &safe_js_string("ownerRetired"));
                map.str_set("ownerKey", &safe_js_string(owner_key));
            }
            HostEvent::FrameChanged { frame } => {
                map.str_set("type", &safe_js_string("frameChanged"));
                map.str_set("frame", &JsValue::from_f64(*frame as f64));
            }
            HostEvent::StageSizeChanged { width, height, center } => {
                map.str_set("type", &safe_js_string("stageSizeChanged"));
                map.str_set("width", &JsValue::from_f64(*width as f64));
                map.str_set("height", &JsValue::from_f64(*height as f64));
                map.str_set("center", &JsValue::from_bool(*center));
            }
            HostEvent::MovieLoaded { version, cast_names } => {
                map.str_set("type", &safe_js_string("movieLoaded"));
                map.str_set("version", &JsValue::from_f64(*version as f64));
                let names = js_sys::Array::new();
                for name in cast_names {
                    names.push(&safe_js_string(name));
                }
                map.str_set("castNames", &names);
            }
            HostEvent::MovieLoadFailed { path, error } => {
                map.str_set("type", &safe_js_string("movieLoadFailed"));
                map.str_set("path", &safe_js_string(path));
                map.str_set("error", &safe_js_string(error));
            }
            HostEvent::ScriptError { message } => {
                map.str_set("type", &safe_js_string("scriptError"));
                map.str_set("message", &safe_js_string(message));
            }
            HostEvent::ScriptErrorCleared => {
                map.str_set("type", &safe_js_string("scriptErrorCleared"));
            }
            HostEvent::CastListChanged { names } => {
                map.str_set("type", &safe_js_string("castListChanged"));
                let values = js_sys::Array::new();
                for name in names {
                    values.push(&safe_js_string(name));
                }
                map.str_set("names", &values);
            }
            HostEvent::CastNameChanged { cast, name } => {
                map.str_set("type", &safe_js_string("castNameChanged"));
                map.str_set("cast", &JsValue::from_f64(*cast as f64));
                map.str_set("name", &safe_js_string(name));
            }
            HostEvent::CastMemberListChanged { cast, members } => {
                map.str_set("type", &safe_js_string("castMemberListChanged"));
                map.str_set("cast", &JsValue::from_f64(*cast as f64));
                let values = js_sys::Array::new();
                for (number, name) in members {
                    let member = js_sys::Map::new();
                    member.str_set("number", &JsValue::from_f64(*number as f64));
                    member.str_set("name", &safe_js_string(name));
                    values.push(&member.to_js_object());
                }
                map.str_set("members", &values);
            }
            HostEvent::FlashReset { owner_key } => {
                map.str_set("type", &safe_js_string("flashReset"));
                map.str_set("ownerKey", &safe_js_string(owner_key));
            }
        }
        map.to_js_object()
    }

    pub fn dispatch_score_snapshot(snapshot: js_sys::Object, owner_key: &str) -> Result<(), JsValue> {
        onScoreChanged(snapshot, owner_key)
    }

    pub fn channel_snapshot_for_player(
        player: &DirPlayer,
        channel: i16,
    ) -> (js_sys::Object, String) {
        (
            Self::get_channel_snapshot(player, &channel).to_js_object(),
            owner_key_string(&player.owner),
        )
    }

    pub fn dispatch_channel_snapshot(
        channel: i16,
        snapshot: js_sys::Object,
        owner_key: &str,
    ) -> Result<(), JsValue> {
        onChannelChanged(channel, snapshot, owner_key)
    }

    pub fn dispatch_channel_changed(channel: i16) {
        let selected_channel = RENDERER_LOCK.with(|x| {
            let borrowed = x.borrow();
            let dynamic = borrowed.as_ref();
            // Check Canvas2D renderer first, then WebGL2
            dynamic
                .and_then(|d| d.as_canvas2d())
                .and_then(|canvas2d| canvas2d.debug_selected_channel_num)
                .or_else(|| {
                    dynamic
                        .and_then(|d| d.as_webgl2())
                        .and_then(|webgl2| webgl2.debug_selected_channel_num)
                })
        });

        if selected_channel == Some(channel) {
            crate::player::spawn_player_local(async move {
                // Deferred task — the player may be gone by the time it runs.
            if unsafe { PLAYER_OPT.is_none() } { return; }
            let player = unsafe { crate::player::player_ref() };
                let snapshot = Self::get_channel_snapshot(player, &channel);
                let owner_key = owner_key_string(&player.owner);
                onChannelChanged(channel, snapshot.to_js_object(), &owner_key);
            });
        }
    }

    pub fn dispatch_frame_changed(frame: u32) {
        onFrameChanged(frame);
    }

    pub fn dispatch_debug_message(message: &str) -> Result<(), JsValue> {
        onDebugMessage(&&safe_string(message))
    }

    pub fn dispatch_debug_message_owned(owner_key: &str, message: &str) -> Result<(), JsValue> {
        onDebugMessageOwned(owner_key, &safe_string(message))
    }

    pub fn dispatch_debug_content(content: js_sys::Object) {
        onDebugContent(content);
    }

    pub fn dispatch_debug_bitmap(width: u32, height: u32, data: &[u8]) {
        let map = js_sys::Map::new();
        map.str_set("type", &safe_js_string("bitmap"));
        map.str_set("width", &JsValue::from_f64(width as f64));
        map.str_set("height", &JsValue::from_f64(height as f64));
        map.str_set("data", &js_sys::Uint8Array::from(data));
        Self::dispatch_debug_content(map.to_js_object());
    }

    pub fn dispatch_debug_datum(
        datum_ref: &DatumRef,
        symbols: &SymbolTable,
        player: &DirPlayer,
    ) {
        let map = js_sys::Map::new();
        map.str_set("type", &safe_js_string("datum"));
        map.str_set("datumRef", &JsValue::from_f64(datum_ref.unwrap() as f64));
        let snapshot = datum_to_js_bridge(datum_ref, symbols, player, 0);
        map.str_set("snapshot", &snapshot);
        Self::dispatch_debug_content(map.to_js_object());
    }

    pub fn get_mini_member_snapshot(member: &CastMember) -> js_sys::Map {
        let member_map = js_sys::Map::new();
        member_map.str_set("name", &safe_js_string(&member.name));
        member_map.str_set("type", &safe_js_string(&member.member_type.type_string()));
        if let CastMemberType::Script(script_data) = &member.member_type {
            member_map.str_set("scriptType", &safe_js_string(match script_data.script_type {
                ScriptType::Movie => "movie",
                ScriptType::Parent => "parent",
                ScriptType::Score => "score",
                _ => "unknown",
            }));
        }
        return member_map;
    }

    pub fn get_member_snapshot(
        member: &CastMember,
        cast_lib: u32,
        lctx: Option<&ScriptContext>,
        symbols: &SymbolTable,
        player: &DirPlayer,
    ) -> js_sys::Map {
        let member_map = js_sys::Map::new();
        member_map.str_set("number", &JsValue::from(member.number));
        member_map.str_set("name", &safe_js_string(&member.name));
        member_map.str_set("type", &safe_js_string(&member.member_type.type_string()));

        match &member.member_type {
            CastMemberType::Field(text_data) => {
                member_map.str_set("text", &ascii_safe(&text_data.text).to_js_value());
            }
            CastMemberType::Text(text_data) => {
                member_map.str_set("text", &ascii_safe(&text_data.text).to_js_value());
                member_map.str_set("htmlSource", &ascii_safe(&text_data.html_source).to_js_value());
                member_map.str_set("alignment", &ascii_safe(text_data.alignment.as_str()).to_js_value());
                member_map.str_set("boxType", &ascii_safe(&text_data.box_type.to_string()).to_js_value());
                member_map.str_set("wordWrap", &JsValue::from_bool(text_data.word_wrap));
                member_map.str_set("antiAlias", &JsValue::from_bool(text_data.anti_alias));
                member_map.str_set("font", &ascii_safe(&text_data.font).to_js_value());
                // set fontStyle array of strings
                let font_style_array = js_sys::Array::new();
                for style in &text_data.font_style {
                    font_style_array.push(&ascii_safe(&style.to_string()).to_js_value());
                }
                member_map.str_set("fontStyle", &font_style_array);
                member_map.str_set("fixedLineSpace", &JsValue::from_f64(text_data.fixed_line_space as f64));
                member_map.str_set("topSpacing", &JsValue::from_f64(text_data.top_spacing as f64));
                member_map.str_set("bottomSpacing", &JsValue::from_f64(text_data.bottom_spacing as f64));
                member_map.str_set("width", &JsValue::from_f64(text_data.width as f64));
                member_map.str_set("height", &JsValue::from_f64(text_data.height as f64));
                // set spans array
                let spans_array = js_sys::Array::new();
                for span in &text_data.html_styled_spans {
                    let span_map = js_sys::Map::new();
                    span_map.str_set("text", &ascii_safe(&span.text).to_js_value());
                    span_map.str_set("fontFace", &ascii_safe(&span.style.font_face.clone().unwrap_or_default()).to_js_value());
                    span_map.str_set("fontSize", &JsValue::from_f64(span.style.font_size.unwrap_or_default() as f64));
                    span_map.str_set("bold", &JsValue::from_bool(span.style.bold));
                    span_map.str_set("italic", &JsValue::from_bool(span.style.italic));
                    span_map.str_set("underline", &JsValue::from_bool(span.style.underline));
                    span_map.str_set("color", &JsValue::from_f64(span.style.color.unwrap_or_default() as f64));
                    spans_array.push(&span_map.to_js_object());
                }
                member_map.str_set("htmlStyledSpans", &spans_array);

            }
            CastMemberType::Script(script_data) => {
                let lctx = lctx.unwrap();
                let script = &lctx.scripts[&script_data.script_id];

                // Get cast info for variable multiplier
                let cast = player
                    .movie
                    .cast_manager
                    .get_cast(cast_lib)
                    .unwrap();
                let capital_x = cast.capital_x;
                let dir_version = cast.dir_version;

                member_map.str_set(
                    "script",
                    &Self::get_script_snapshot(
                        &script_data,
                        &script,
                        &lctx,
                        capital_x,
                        dir_version,
                        symbols,
                    ).to_js_object(),
                );
            }
            CastMemberType::Bitmap(bitmap_data) => {
                let bitmap = player
                    .bitmap_manager
                    .get_bitmap(bitmap_data.image_ref)
                    .unwrap();
                member_map.str_set("width", &JsValue::from(bitmap.width));
                member_map.str_set("height", &JsValue::from(bitmap.height));
                member_map.str_set("bitDepth", &JsValue::from(bitmap.bit_depth));
                member_map.str_set("paletteRef", &bitmap.palette_ref.to_js_value());
                member_map.str_set("regX", &JsValue::from(bitmap_data.reg_point.0));
                member_map.str_set("regY", &JsValue::from(bitmap_data.reg_point.1));
            }
            CastMemberType::Sound(sound_member) => {
                member_map.str_set("sampleRate", &JsValue::from(sound_member.info.sample_rate));
                member_map.str_set("channels", &JsValue::from(sound_member.info.channels));
                member_map.str_set("bitsPerSample", &JsValue::from(sound_member.info.sample_size));
                member_map.str_set("sampleCount", &JsValue::from(sound_member.info.sample_count));
                member_map.str_set("duration", &JsValue::from(sound_member.info.duration));
                member_map.str_set("loop", &JsValue::from_bool(sound_member.info.loop_enabled));
                member_map.str_set("codec", &safe_js_string(&sound_member.sound.codec()));
                member_map.str_set("dataSize", &JsValue::from(sound_member.sound.data().len() as u32));
            }
            CastMemberType::FilmLoop(film_loop_data) => {
                member_map.str_set("width", &JsValue::from(film_loop_data.info.width));
                member_map.str_set("height", &JsValue::from(film_loop_data.info.height));
                member_map.str_set("center", &JsValue::from(film_loop_data.info.center));
                member_map.str_set("regX", &JsValue::from(film_loop_data.info.reg_point.0));
                member_map.str_set("regY", &JsValue::from(film_loop_data.info.reg_point.1));
                let score_snapshot = Self::get_score_snapshot(player, &film_loop_data.score);
                member_map.str_set("score", &score_snapshot.to_js_object());
            }
            CastMemberType::Flash(flash_data) => {
                member_map.str_set("regX", &JsValue::from(flash_data.reg_point.0));
                member_map.str_set("regY", &JsValue::from(flash_data.reg_point.1));
                member_map.str_set("dataSize", &JsValue::from(flash_data.data.len() as u32));
                if let Some(ref info) = flash_data.flash_info {
                    member_map.str_set("flashRectLeft", &JsValue::from(info.flash_rect.0));
                    member_map.str_set("flashRectTop", &JsValue::from(info.flash_rect.1));
                    member_map.str_set("flashRectRight", &JsValue::from(info.flash_rect.2));
                    member_map.str_set("flashRectBottom", &JsValue::from(info.flash_rect.3));
                    member_map.str_set("width", &JsValue::from(info.flash_rect.2 - info.flash_rect.0));
                    member_map.str_set("height", &JsValue::from(info.flash_rect.3 - info.flash_rect.1));
                    member_map.str_set("directToStage", &JsValue::from_bool(info.direct_to_stage));
                    member_map.str_set("imageEnabled", &JsValue::from_bool(info.image_enabled));
                    member_map.str_set("soundEnabled", &JsValue::from_bool(info.sound_enabled));
                    member_map.str_set("pausedAtStart", &JsValue::from_bool(info.paused_at_start));
                    member_map.str_set("loop", &JsValue::from_bool(info.loop_enabled));
                    member_map.str_set("isStatic", &JsValue::from_bool(info.is_static));
                    member_map.str_set("preload", &JsValue::from_bool(info.preload));
                    member_map.str_set("centerRegPoint", &JsValue::from_bool(info.center_reg_point));
                    member_map.str_set("buttonsEnabled", &JsValue::from_bool(info.buttons_enabled));
                    member_map.str_set("actionsEnabled", &JsValue::from_bool(info.actions_enabled));
                    member_map.str_set("fixedRate", &JsValue::from(info.fixed_rate));
                    member_map.str_set("posterFrame", &JsValue::from(info.poster_frame));
                    member_map.str_set("bufferSize", &JsValue::from(info.buffer_size));
                    member_map.str_set("scale", &JsValue::from_f64(info.scale as f64));
                    member_map.str_set("viewScale", &JsValue::from_f64(info.view_scale as f64));
                    member_map.str_set("originH", &JsValue::from_f64(info.origin_h as f64));
                    member_map.str_set("originV", &JsValue::from_f64(info.origin_v as f64));
                    member_map.str_set("viewH", &JsValue::from_f64(info.view_h as f64));
                    member_map.str_set("viewV", &JsValue::from_f64(info.view_v as f64));
                    member_map.str_set("originMode", &safe_js_string(match info.origin_mode {
                        crate::director::enums::FlashOriginMode::Center => "center",
                        crate::director::enums::FlashOriginMode::TopLeft => "topLeft",
                        crate::director::enums::FlashOriginMode::Point => "point",
                    }));
                    member_map.str_set("playbackMode", &safe_js_string(match info.playback_mode {
                        crate::director::enums::FlashPlaybackMode::Normal => "normal",
                        crate::director::enums::FlashPlaybackMode::Fixed => "fixed",
                        crate::director::enums::FlashPlaybackMode::LockStep => "lockStep",
                    }));
                    member_map.str_set("scaleMode", &safe_js_string(match info.scale_mode {
                        crate::director::enums::FlashScaleMode::ShowAll => "showAll",
                        crate::director::enums::FlashScaleMode::NoScale => "noScale",
                        crate::director::enums::FlashScaleMode::AutoSize => "autoSize",
                        crate::director::enums::FlashScaleMode::ExactFit => "exactFit",
                        crate::director::enums::FlashScaleMode::NoBorder => "noBorder",
                    }));
                    member_map.str_set("streamMode", &safe_js_string(match info.stream_mode {
                        crate::director::enums::FlashStreamMode::Frame => "frame",
                        crate::director::enums::FlashStreamMode::Idle => "idle",
                        crate::director::enums::FlashStreamMode::Manual => "manual",
                    }));
                    member_map.str_set("quality", &safe_js_string(match info.quality {
                        crate::director::enums::FlashQuality::AutoHigh => "autoHigh",
                        crate::director::enums::FlashQuality::AutoMedium => "autoMedium",
                        crate::director::enums::FlashQuality::AutoLow => "autoLow",
                        crate::director::enums::FlashQuality::High => "high",
                        crate::director::enums::FlashQuality::Medium => "medium",
                        crate::director::enums::FlashQuality::Low => "low",
                    }));
                    member_map.str_set("eventPassMode", &safe_js_string(match info.event_pass_mode {
                        crate::director::enums::FlashEventPassMode::PassAlways => "passAlways",
                        crate::director::enums::FlashEventPassMode::PassButton => "passButton",
                        crate::director::enums::FlashEventPassMode::PassNotButton => "passNotButton",
                        crate::director::enums::FlashEventPassMode::PassNever => "passNever",
                    }));
                    member_map.str_set("clickMode", &safe_js_string(match info.click_mode {
                        crate::director::enums::FlashClickMode::BoundingBox => "boundingBox",
                        crate::director::enums::FlashClickMode::Opaque => "opaque",
                        crate::director::enums::FlashClickMode::Object => "object",
                    }));
                    member_map.str_set("sourceFileName", &safe_js_string(&info.source_file_name));
                    member_map.str_set("commonPlayer", &safe_js_string(&info.common_player));
                    member_map.str_set("bgColor", &JsValue::from(info.bg_color));
                }
            }
            CastMemberType::Palette(palette) => {
                let colors_array = js_sys::Array::new();
                for color in palette.colors.iter() {
                    let color_array = js_sys::Array::new();
                    color_array.push(&JsValue::from_f64(color.0 as f64));
                    color_array.push(&JsValue::from_f64(color.1 as f64));
                    color_array.push(&JsValue::from_f64(color.2 as f64));
                    colors_array.push(&color_array);
                }
                member_map.str_set("colors", &colors_array);
            }
            CastMemberType::Shockwave3d(s3d_data) => {
                let info = &s3d_data.info;
                member_map.str_set("regX", &JsValue::from(info.reg_point.0));
                member_map.str_set("regY", &JsValue::from(info.reg_point.1));
                member_map.str_set("dataSize", &JsValue::from(s3d_data.w3d_data.len() as u32));
                member_map.str_set("directToStage", &JsValue::from_bool(info.direct_to_stage));
                member_map.str_set("animationEnabled", &JsValue::from_bool(info.animation_enabled));
                member_map.str_set("preload", &JsValue::from_bool(info.preload));
                member_map.str_set("loop", &JsValue::from_bool(info.loops));
                member_map.str_set("duration", &JsValue::from(info.duration));
                let rect = info.default_rect;
                member_map.str_set("width", &JsValue::from(rect.2 - rect.0));
                member_map.str_set("height", &JsValue::from(rect.3 - rect.1));
                member_map.str_set("rectLeft", &JsValue::from(rect.0));
                member_map.str_set("rectTop", &JsValue::from(rect.1));
                member_map.str_set("rectRight", &JsValue::from(rect.2));
                member_map.str_set("rectBottom", &JsValue::from(rect.3));
                if let Some(pos) = info.camera_position {
                    let arr = js_sys::Array::new();
                    arr.push(&JsValue::from_f64(pos.0 as f64));
                    arr.push(&JsValue::from_f64(pos.1 as f64));
                    arr.push(&JsValue::from_f64(pos.2 as f64));
                    member_map.str_set("cameraPosition", &arr);
                }
                if let Some(rot) = info.camera_rotation {
                    let arr = js_sys::Array::new();
                    arr.push(&JsValue::from_f64(rot.0 as f64));
                    arr.push(&JsValue::from_f64(rot.1 as f64));
                    arr.push(&JsValue::from_f64(rot.2 as f64));
                    member_map.str_set("cameraRotation", &arr);
                }
                if let Some(bg) = info.bg_color {
                    member_map.str_set("bgColor", &safe_js_string(&format!("rgb({},{},{})", bg.0, bg.1, bg.2)));
                }
                if let Some(ambient) = info.ambient_color {
                    member_map.str_set("ambientColor", &safe_js_string(&format!("rgb({},{},{})", ambient.0, ambient.1, ambient.2)));
                }
                member_map.str_set("hasScene", &JsValue::from_bool(s3d_data.parsed_scene.is_some()));
            }
            _ => {}
        };

        return member_map;
    }

    pub fn get_score_snapshot(player: &DirPlayer, score: &Score) -> js_sys::Map {
        let member_map = js_sys::Map::new();
        member_map.str_set("channelCount", &JsValue::from(score.get_channel_count()));
        // The score's own frame count, already reconciled from the chunk header,
        // sprite spans, init data, keyframes and frame labels. The UI needs it to
        // size the timeline; deriving it again in JS would only see the subset of
        // that information the snapshot happens to carry.
        member_map.str_set("frameCount", &JsValue::from(score.frame_count.unwrap_or(1)));

        member_map.str_set(
            "behaviorReferences",
            &js_sys::Array::from_iter(
                score
                    .sprite_spans
                    .iter()
                    .filter(|span| span.scripts.len() > 0)
                    .map(|span| {
                        let behavior = span.scripts.first().unwrap();
                        let script_ref_map = js_sys::Map::new();
                        script_ref_map.str_set("startFrame", &span.start_frame.to_js_value());
                        script_ref_map.str_set("endFrame", &span.end_frame.to_js_value());
                        script_ref_map.str_set("castLib", &behavior.cast_lib.to_js_value());
                        script_ref_map.str_set("castMember", &behavior.cast_member.to_js_value());
                        script_ref_map.str_set("channelNumber", &span.channel_number.to_js_value());
                        script_ref_map.str_set(
                            "memberName",
                            &safe_js_string(&Self::lookup_member_name(
                                player,
                                [behavior.cast_lib as u16, behavior.cast_member as u16],
                            )),
                        );
                        JsValue::from(script_ref_map.to_js_object())
                    }),
            ),
        );

        // Build sprite spans from the raw channel data
        let sprite_spans = Self::create_sprite_spans_from_channels(score, player);
        
        member_map.str_set(
            "spriteSpans",
            &js_sys::Array::from_iter(sprite_spans.iter().map(|span| span.to_js_value())),
        );
        
        return member_map;
    }

    /// Member name for a [cast_lib, cast_member] pair; empty when unnamed or
    /// unresolvable. One map lookup per span, done once per score snapshot.
    fn lookup_member_name(player: &DirPlayer, member_ref: [u16; 2]) -> String {
        if member_ref[0] == 0 && member_ref[1] == 0 {
            return String::new();
        }
        let reference = CastMemberRef {
            cast_lib: member_ref[0] as i32,
            cast_member: member_ref[1] as i32,
        };
        player
            .movie
            .cast_manager
            .find_member_by_ref(&reference)
            .map(|member| member.name.clone())
            .unwrap_or_default()
    }

    // Create sprite spans by examining actual channel state across frames
    fn create_sprite_spans_from_channels(score: &Score, player: &DirPlayer) -> Vec<ScoreSpriteSpan> {
        use std::collections::HashMap;
        
        let mut spans = Vec::new();
        let mut channel_data: HashMap<u16, Vec<(u32, u16, u16)>> = HashMap::new();
        
        // Collect all frame data per channel from channel_initialization_data.
        //
        // channel_initialization_data is keyed by a 0-based frame *index*, but
        // everything else that crosses to JS — behaviorReferences, the current
        // frame, the timeline's own columns — uses Director's 1-based frame
        // *numbers*. Normalise here so the snapshot speaks one language; before
        // this, spans came out as 0..27 for a 28-frame movie, so the UI's
        // `frame === span.startFrame` test never matched and sprite labels
        // never rendered.
        for (frame_index, channel_index, init_data) in &score.channel_initialization_data {
            let frame_index = &(frame_index + 1);
            let channel_num = get_channel_number_from_index(*channel_index as u32) as u16;
            let cast_lib = init_data.cast_lib;
            let cast_member = init_data.cast_member;
            
            // Skip empty sprites
            if cast_lib == 0 && cast_member == 0 {
                continue;
            }
            
            channel_data
                .entry(channel_num)
                .or_insert_with(Vec::new)
                .push((*frame_index, cast_lib, cast_member));
        }
        
        // For each channel, create spans from consecutive frames
        for (channel_num, mut frames) in channel_data {
            // Sort by frame
            frames.sort_by_key(|(frame, _, _)| *frame);
            
            let mut current_span: Option<ScoreSpriteSpan> = None;
            
            for (frame, cast_lib, cast_member) in frames {
                let member_ref = [cast_lib, cast_member];
                
                if let Some(ref mut span) = current_span {
                    // Check if this continues the current span
                    if span.member_ref == member_ref && span.end_frame + 1 == frame {
                        // Extend the current span
                        span.end_frame = frame;
                    } else {
                        // Save the current span and start a new one
                        spans.push(span.clone());
                        current_span = Some(ScoreSpriteSpan {
                            channel_number: channel_num,
                            start_frame: frame,
                            end_frame: frame,
                            member_ref,
                            member_name: Self::lookup_member_name(player, member_ref),
                        });
                    }
                } else {
                    // Start the first span for this channel
                    current_span = Some(ScoreSpriteSpan {
                        channel_number: channel_num,
                        start_frame: frame,
                        end_frame: frame,
                        member_ref,
                        member_name: Self::lookup_member_name(player, member_ref),
                    });
                }
            }
            
            // Don't forget the last span!
            if let Some(span) = current_span {
                spans.push(span);
            }
        }
        
        // Sort spans by channel, then start frame
        spans.sort_by_key(|s| (s.channel_number, s.start_frame));
        
        spans
    }

    /// Every channel's display name in one message.
    ///
    /// Subscribing used to emit one callback per channel, and a movie has ~1000
    /// of them: 1000 boundary crossings, 1000 Redux actions, and a reducer that
    /// copies the whole channel map each time — quadratic, and it blocked the
    /// main thread for over two seconds when the Channels or Timeline panel was
    /// first opened. Named channels only; the empty ones carry no information.
    pub fn dispatch_all_channel_names(player: &DirPlayer) {
        let (names, owner_key) = Self::channel_names_snapshot_for_player(player);
        Self::dispatch_channel_names_snapshot(names, &owner_key);
    }

    pub fn channel_names_snapshot_for_player(
        player: &DirPlayer,
    ) -> (js_sys::Object, String) {
        let names = js_sys::Map::new();
        for channel in &player.movie.score.channels {
            let number = channel.number as i16;
            if let Some(display_name) = Self::get_channel_display_name(&number, player) {
                if display_name.is_empty() { continue; }
                names.set(&JsValue::from(number), &JsValue::from(display_name));
            }
        }
        let owner_key = owner_key_string(&player.owner);
        (names.to_js_object(), owner_key)
    }

    pub fn dispatch_channel_names_snapshot(
        names: js_sys::Object,
        owner_key: &str,
    ) -> Result<(), JsValue> {
        onChannelDisplayNamesChanged(names, owner_key)
    }

    pub fn dispatch_channel_name_snapshot(
        channel: i16,
        name: &str,
        owner_key: &str,
    ) -> Result<(), JsValue> {
        onChannelDisplayNameChanged(channel, name, owner_key)
    }

    /// Extract owned channel-name payloads affected by one cast slot. The
    /// session dispatches these only after releasing its player borrow.
    pub fn channel_name_snapshots_for_member_slot(
        player: &DirPlayer,
        slot_number: u32,
    ) -> Vec<(i16, String)> {
        if !player.is_subscribed_to_channel_names {
            return Vec::new();
        }
        player
            .movie
            .score
            .channels
            .iter()
            .filter_map(|channel| {
                let matches = channel.sprite.member.as_ref().map(|member| {
                    CastMemberRefHandlers::get_cast_slot_number(
                        member.cast_lib as u32,
                        member.cast_member as u32,
                    )
                }) == Some(slot_number);
                if !matches {
                    return None;
                }
                Some((
                    channel.number as i16,
                    Self::get_channel_display_name(&(channel.number as i16), player)
                        .unwrap_or_default(),
                ))
            })
            .collect()
    }

    pub fn channel_name_snapshot_for_player(
        player: &DirPlayer,
        channel: i16,
    ) -> Option<(String, String)> {
        if !player.is_subscribed_to_channel_names {
            return None;
        }
        if channel < 0 || channel as usize >= player.movie.score.channels.len() {
            return None;
        }
        Some((
            Self::get_channel_display_name(&channel, player).unwrap_or_default(),
            owner_key_string(&player.owner),
        ))
    }

    pub fn dispatch_channel_name_changed(channel: i16) {
        crate::player::spawn_player_local(async move {
            // Deferred task — the player may be gone by the time it runs.
            if unsafe { PLAYER_OPT.is_none() } { return; }
            let player = unsafe { crate::player::player_ref() };

            if player.is_subscribed_to_channel_names {
                let display_name =
                    Self::get_channel_display_name(&channel, player).unwrap_or("".to_owned());
                let owner_key = owner_key_string(&player.owner);
                onChannelDisplayNameChanged(channel, &display_name, &owner_key);
            }
        });
    }

    fn get_channel_display_name(channel: &i16, player: &DirPlayer) -> Option<String> {
        let channel = player.movie.score.channels.get(*channel as usize)?;
        let member_ref = &channel.sprite.member.as_ref();
        if member_ref.is_none() || !member_ref.unwrap().is_valid() {
            return None;
        }
        let member_ref = member_ref.unwrap();
        let member = player.movie.cast_manager.find_member_by_ref(&member_ref);
        if member.is_none() {
            return None;
        }
        let member = member.unwrap();

        // Use safe_string to handle non-ASCII characters (e.g., Japanese)
        if !channel.name.is_empty() {
            return Some(safe_string(&channel.name));
        } else if !channel.sprite.name.is_empty() {
            return Some(safe_string(&channel.sprite.name));
        } else if !member.name.is_empty() {
            return Some(safe_string(&member.name));
        } else {
            return None;
        }
    }

    pub fn get_channel_snapshot(player: &DirPlayer, channel_num: &i16) -> js_sys::Map {
        let result = js_sys::Map::new();
        let Some(channel) = (*channel_num >= 0)
            .then(|| player.movie.score.channels.get(*channel_num as usize))
            .flatten()
        else {
            debug!("get_channel_snapshot: channel {} is outside the loaded score", channel_num);
            return result;
        };

        let member_ref = &channel.sprite.member.as_ref();
        if member_ref.is_none() || !member_ref.unwrap().is_valid() {
            debug!(
                "get_channel_snapshot: ch{} member_ref is None or invalid, puppet={}",
                channel_num, channel.sprite.puppet
            );
            return result;
        }
        let member_ref = member_ref.unwrap();
        let display_name =
            Self::get_channel_display_name(channel_num, player).unwrap_or("".to_owned());

        let member_ref_array = js_sys::Array::new();
        member_ref_array.push(&JsValue::from_f64(member_ref.cast_lib as f64));
        member_ref_array.push(&JsValue::from_f64(member_ref.cast_member as f64));

        let script_instance_array = js_sys::Array::new();
        for script_instance in &channel.sprite.script_instance_list {
            script_instance_array.push(&JsValue::from_f64(**script_instance as f64));
        }

        let sprite_map = js_sys::Map::new();
        sprite_map.str_set("displayName", &display_name.to_js_value());
        sprite_map.str_set("memberRef", &member_ref_array);
        sprite_map.str_set("scriptInstanceList", &script_instance_array);
        sprite_map.str_set("width", &JsValue::from_f64(channel.sprite.width as f64));
        sprite_map.str_set("height", &JsValue::from_f64(channel.sprite.height as f64));
        sprite_map.str_set("locH", &JsValue::from_f64(channel.sprite.loc_h as f64));
        sprite_map.str_set("locV", &JsValue::from_f64(channel.sprite.loc_v as f64));
        sprite_map.str_set("color", &channel.sprite.color.to_string().to_js_value());
        sprite_map.str_set(
            "bgColor",
            &channel.sprite.bg_color.to_string().to_js_value(),
        );
        sprite_map.str_set("ink", &JsValue::from_f64(channel.sprite.ink as f64));
        sprite_map.str_set("blend", &JsValue::from_f64(channel.sprite.blend as f64));

        return sprite_map;
    }

    /// Emit one synthetic "handler" entry per JS function in `ir`. The
    /// top-level program body is also emitted (under the name "(toplevel)")
    /// because it carries the script's var initializers and any
    /// non-function top-level statements — important for diagnosing
    /// missing-constant bugs (bi_bpe, bi_mask, etc.).
    fn push_js_handlers(
        ir: &crate::player::js_lingo::JsScriptIR,
        path_prefix: &str,
        out: &js_sys::Array,
    ) {
        use crate::player::js_lingo::xdr::JsAtom;

        if path_prefix.is_empty() {
            Self::push_one_js_handler(ir, "(toplevel)", &[], out);
        }

        for atom in &ir.atoms {
            if let JsAtom::Function(fa) = atom {
                let handler_name = match (path_prefix.is_empty(), &fa.name) {
                    (true, Some(n)) => n.clone(),
                    (true, None) => "(anonymous)".to_string(),
                    (false, Some(n)) => format!("{}.{}", path_prefix, n),
                    (false, None) => format!("{}.(anonymous)", path_prefix),
                };
                let arg_names: Vec<String> = fa.bindings.iter()
                    .filter(|b| b.kind == crate::player::js_lingo::xdr::JsBindingKind::Argument)
                    .map(|b| b.name.clone())
                    .collect();
                Self::push_one_js_handler_with_bindings(&fa.script, &handler_name, &arg_names, &fa.bindings, out);
                // Only drill into nested closures if this function actually
                // declares some (i.e. its atom map contains JsAtom::Function).
                let has_nested = fa.script.atoms.iter().any(|a| matches!(a, JsAtom::Function(_)));
                if has_nested {
                    Self::push_js_handlers(&fa.script, &handler_name, out);
                }
            }
        }
    }

    fn push_one_js_handler(
        ir: &crate::player::js_lingo::JsScriptIR,
        name: &str,
        arg_names: &[String],
        out: &js_sys::Array,
    ) {
        Self::push_one_js_handler_with_bindings(ir, name, arg_names, &[], out);
    }

    fn push_one_js_handler_with_bindings(
        ir: &crate::player::js_lingo::JsScriptIR,
        name: &str,
        arg_names: &[String],
        bindings: &[crate::player::js_lingo::xdr::JsFunctionBinding],
        out: &js_sys::Array,
    ) {
        use crate::player::js_lingo::opcodes::JsOpFormat;
        use crate::player::js_lingo::variable_length::{read_i16_operand, read_u16_operand};
        use crate::player::js_lingo::xdr::iter_ops;

        let handler_map = js_sys::Map::new();
        handler_map.str_set("name", &name.to_owned().to_js_value());

        let args_array = js_sys::Array::new();
        for a in arg_names { args_array.push(&a.clone().to_js_value()); }
        handler_map.str_set("args", &args_array);

        let bytecode_array = js_sys::Array::new();
        let lingo_array = js_sys::Array::new();
        let mut bc_to_line: Vec<(usize, usize)> = Vec::new();

        // Decompile to JS source — this is what the "Lingo" tab displays.
        let decomp = crate::player::js_lingo::decompiler::decompile(ir, bindings);
        let decomp_bc_to_line: std::collections::HashMap<usize, usize> =
            decomp.bytecode_to_line.iter().copied().collect();
        for line in &decomp.lines {
            let line_map = js_sys::Map::new();
            line_map.str_set("text", &line.text.clone().to_js_value());
            line_map.str_set("indent", &JsValue::from(line.indent));
            let idx_arr = js_sys::Array::new();
            for bc in &line.bytecode_indices {
                idx_arr.push(&JsValue::from(*bc as u32));
            }
            line_map.str_set("bytecodeIndices", &idx_arr);
            line_map.str_set("spans", &js_sys::Array::new());
            lingo_array.push(&line_map.to_js_object());
        }

        for (i, ins) in iter_ops(&ir.bytecode).enumerate() {
            let ins = match ins {
                Ok(i) => i,
                Err(e) => {
                    let map = js_sys::Map::new();
                    map.str_set("pos", &JsValue::from(0u32));
                    map.str_set("text", &format!("<decode error: {}>", e).to_js_value());
                    bytecode_array.push(&map.to_js_object());
                    continue;
                }
            };
            let info = ins.op.info();
            let operand_str = match info.format {
                JsOpFormat::Byte => String::new(),
                JsOpFormat::Uint16 | JsOpFormat::Qarg | JsOpFormat::Qvar | JsOpFormat::Local => {
                    read_u16_operand(ins.operand).map(|v| format!(" {}", v)).unwrap_or_default()
                }
                JsOpFormat::Const => {
                    if let Ok(idx) = read_u16_operand(ins.operand) {
                        let lbl = ir.atoms.get(idx as usize).map(format_atom_summary).unwrap_or_else(|| "<oob>".into());
                        format!(" #{} ; {}", idx, lbl)
                    } else { String::new() }
                }
                JsOpFormat::Jump => {
                    read_i16_operand(ins.operand).map(|d| format!(" {:+} ; -> {}", d, ins.offset as i32 + d as i32)).unwrap_or_default()
                }
                JsOpFormat::Object => {
                    read_u16_operand(ins.operand).map(|v| format!(" obj#{}", v)).unwrap_or_default()
                }
                JsOpFormat::Tableswitch | JsOpFormat::Lookupswitch => format!(" <{} bytes>", ins.operand.len()),
            };
            let text = format!("{:>4}: {:<14}{}", ins.offset, info.mnemonic, operand_str);
            let map = js_sys::Map::new();
            map.str_set("pos", &JsValue::from(ins.offset as u32));
            map.str_set("text", &text.to_js_value());
            bytecode_array.push(&map.to_js_object());

            // bc → line mapping: use the decompiler's mapping. Any bytecode
            // not covered (e.g. structural markers like POP after Pushobj)
            // points at the nearest source line if we have one, else line 0.
            if let Some(line_idx) = decomp_bc_to_line.get(&i) {
                bc_to_line.push((i, *line_idx));
            }
        }
        let _ = bc_to_line; // populated for completeness; the map below uses decomp_bc_to_line directly
        handler_map.str_set("bytecode", &bytecode_array);
        handler_map.str_set("lingo", &lingo_array);
        let mapping_obj = js_sys::Object::new();
        for (bc, ln) in &decomp_bc_to_line {
            js_sys::Reflect::set(&mapping_obj, &JsValue::from(*bc as u32), &JsValue::from(*ln as u32)).ok();
        }
        handler_map.str_set("bytecodeToLine", &mapping_obj);
        out.push(&handler_map.to_js_object());
    }

    pub fn get_script_snapshot(
        member: &ScriptMember,
        chunk: &ScriptChunk,
        lctx: &ScriptContext,
        capital_x: bool,
        dir_version: u16,
        symbols: &SymbolTable,
    ) -> js_sys::Map {
        let member_map = js_sys::Map::new();
        member_map.str_set("name", &member.name.to_js_value());
        member_map.str_set(
            "script_type",
            &match member.script_type {
                ScriptType::Movie => "movie".to_owned().to_js_value(),
                ScriptType::Parent => "parent".to_owned().to_js_value(),
                ScriptType::Score => "score".to_owned().to_js_value(),
                _ => "unknown".to_owned().to_js_value(),
            },
        );

        // JS-Lingo path: the Lscr's literal-data region holds an XDR-serialized
        // SpiderMonkey script. We replace the handler array with a synthetic
        // one whose "bytecode" view is the JS disassembly and whose "lingo"
        // view is a placeholder comment header. Lingo handlers in this
        // chunk (if any) get appended afterwards.
        if let Some(js_payload) = chunk.literals.iter().find_map(|l| match l {
            crate::director::lingo::datum::Datum::JavaScript(b) => Some(b.as_slice()),
            _ => None,
        }) {
            member_map.str_set("script_syntax", &"javascript".to_owned().to_js_value());
            if let Ok(ir) = crate::player::js_lingo::decode_script(js_payload) {
                let handlers_array = js_sys::Array::new();
                Self::push_js_handlers(&ir, "", &handlers_array);
                member_map.str_set("handlers", &handlers_array);
                return member_map;
            }
        }

        member_map.str_set("script_syntax", &"lingo".to_owned().to_js_value());

        // Calculate multiplier once
        let multiplier = get_variable_multiplier(capital_x, dir_version);

        let handlers_array = js_sys::Array::new();
        for handler in &chunk.handlers {
            let handler_map = js_sys::Map::new();
            let bytecode_array = js_sys::Array::new();
            let args_array = js_sys::Array::new();
            // name_id 0xFFFF (or any out-of-range id) is an anonymous handler
            // slot; show a placeholder rather than indexing past the names
            // table (panicked on a netjack D4 script).
            let anon = "<anonymous>".to_string();
            let name = lctx.names.get(handler.name_id as usize).unwrap_or(&anon);

            for bytecode in &handler.bytecode_array {
                let bytecode_map = js_sys::Map::new();

                bytecode_map.str_set("pos", &JsValue::from(bytecode.pos));
                bytecode_map.str_set(
                    "text",
                    &bytecode.to_bytecode_text(lctx, &handler, multiplier).to_js_value(),
                );

                bytecode_array.push(&bytecode_map.to_js_object());
            }

            for arg in &handler.argument_name_ids {
                if let Some(arg_name) = lctx.names.get(*arg as usize) {
                    args_array.push(&arg_name.to_js_value());
                }
            }

            handler_map.str_set("name", &name.to_js_value());
            handler_map.str_set("args", &args_array);
            handler_map.str_set("bytecode", &bytecode_array);

            // Decompile handler to Lingo source
            let decompiled = match decompiler::decompile_handler(
                handler,
                chunk,
                lctx,
                dir_version,
                multiplier,
                symbols,
            ) {
                Ok(decompiled) => decompiled,
                Err(error) => {
                    handler_map.str_set("error", &safe_js_string(&error.message));
                    handler_map.str_set("lingo", &js_sys::Array::new());
                    handler_map.str_set("bytecodeToLine", &js_sys::Object::new());
                    handlers_array.push(&handler_map.to_js_object());
                    continue;
                }
            };

            // Add lingo lines
            let lingo_array = js_sys::Array::new();
            for line in &decompiled.lines {
                let line_map = js_sys::Map::new();
                line_map.str_set("text", &line.text.to_js_value());
                line_map.str_set("indent", &JsValue::from(line.indent));

                let indices_array = js_sys::Array::new();
                for &idx in &line.bytecode_indices {
                    indices_array.push(&JsValue::from(idx as u32));
                }
                line_map.str_set("bytecodeIndices", &indices_array);

                // Add syntax highlighting spans
                let spans_array = js_sys::Array::new();
                for span in &line.spans {
                    let span_map = js_sys::Map::new();
                    span_map.str_set("text", &span.text.to_js_value());
                    span_map.str_set("type", &JsValue::from_str(span.token_type.as_str()));
                    spans_array.push(&span_map.to_js_object());
                }
                line_map.str_set("spans", &spans_array);

                lingo_array.push(&line_map.to_js_object());
            }
            handler_map.str_set("lingo", &lingo_array);

            // Add bytecode to line mapping
            let mapping_obj = js_sys::Object::new();
            for (&bc_idx, &line_idx) in &decompiled.bytecode_to_line {
                js_sys::Reflect::set(
                    &mapping_obj,
                    &JsValue::from(bc_idx as u32),
                    &JsValue::from(line_idx as u32),
                ).ok();
            }
            handler_map.str_set("bytecodeToLine", &mapping_obj);

            handlers_array.push(&handler_map.to_js_object());
        }
        member_map.str_set("handlers", &handlers_array);

        return member_map;
    }

    pub fn dispatch_scope_list(player: &mut DirPlayer) {
        let scope_indices: Vec<usize> = player
            .scopes
            .iter()
            .enumerate()
            .filter(|(i, _)| player.scope_count > *i as u32)
            .map(|(index, _)| index)
            .collect();
        onScopeListChanged(
            scope_indices.into_iter().map(|index| {
                    let (script_ref, handler_name_id, bytecode_index, local_refs, stack_refs, args) = {
                        let (scopes, allocator, bitmap_manager) = (
                            &mut player.scopes,
                            &mut player.allocator,
                            &mut player.bitmap_manager,
                        );
                        let scope = &mut scopes[index];
                        let local_refs = scope
                            .locals
                            .iter()
                            .cloned()
                            .map(|value| value.into_ref_with(allocator, bitmap_manager))
                            .collect::<Vec<_>>();
                        let stack_refs = scope
                            .stack
                            .snapshot_refs_with(allocator, bitmap_manager);
                        (
                            scope.script_ref.clone(),
                            scope.handler_name_id,
                            scope.bytecode_index,
                            local_refs,
                            stack_refs,
                            scope.args.clone(),
                        )
                    };
                    let cast_lib = player
                        .movie
                        .cast_manager
                        .get_cast(script_ref.cast_lib as u32)
                        .unwrap();
                    let handler_name = cast_lib
                        .lctx
                        .as_ref()
                        .unwrap()
                        .names
                        .get(handler_name_id as usize)
                        .unwrap();
                    let names = &cast_lib.lctx.as_ref().unwrap().names;
                    let scope = JsBridgeScope {
                        script_member_ref: script_ref.to_js(),
                        script_member_name: Self::lookup_member_name(
                            player,
                            [script_ref.cast_lib as u16, script_ref.cast_member as u16],
                        ),
                        bytecode_index: bytecode_index as u32,
                        handler_name: handler_name.to_owned(),
                        // Locals are slot-indexed, so recover the name via
                        // the handler's local table: slot -> name id -> name.
                        locals: {
                            let local_name_ids = player
                                .movie
                                .cast_manager
                                .get_script_by_ref(&script_ref)
                                .and_then(|s| s.get_own_handler_by_local_name_id(handler_name_id)
                                    .map(|h| h.local_name_ids.clone()))
                                .unwrap_or_default();
                            local_refs
                                .iter()
                                .enumerate()
                                .map(|(slot, v)| {
                                    let name = local_name_ids
                                        .get(slot)
                                        .and_then(|nid| names.get(*nid as usize).cloned())
                                        .unwrap_or_else(|| format!("local_{}", slot));
                                    (name, v.clone())
                                })
                                .collect()
                        },
                        stack: stack_refs,
                        args,
                        // Same slot -> name id -> name walk the locals use.
                        arg_names: {
                            let arg_name_ids = player
                                .movie
                                .cast_manager
                                .get_script_by_ref(&script_ref)
                                .and_then(|s| s.get_own_handler_by_local_name_id(handler_name_id)
                                    .map(|h| h.argument_name_ids.clone()))
                                .unwrap_or_default();
                            arg_name_ids
                                .iter()
                                .map(|nid| names.get(*nid as usize).cloned().unwrap_or_default())
                                .collect()
                        },
                    };
                    let scope_js: js_sys::Map = scope.into();
                    scope_js.to_js_object()
                })
                .collect(),
        );
    }

    pub fn dispatch_global_list(symbols: &SymbolTable, player: &DirPlayer) {
        let globals = js_sys::Map::new();
        for (k, v) in player.globals.iter() {
            let Ok(key) = symbols.display(k) else {
                log::warn!("skipping foreign global symbol during JS snapshot");
                continue;
            };
            globals.set(&safe_js_string(&ascii_safe(key)), &v.unwrap().to_js_value());
        }
        onGlobalListChanged(globals.to_js_object());
    }

    pub fn dispatch_debug_update(symbols: &SymbolTable, player: &mut DirPlayer) {
        Self::dispatch_scope_list(player);
        Self::dispatch_global_list(symbols, player);
    }

    pub fn script_error_data(player: &DirPlayer, err: &ScriptError) -> js_sys::Object {
        let is_paused = player.current_breakpoint.as_ref()
            .map(|bp| bp.error.is_some())
            .unwrap_or(false);

        let data: js_sys::Map =
            if let Some(current_scope) = player.scopes.get(player.current_scope_ref()) {
                // Best-effort handler-name resolution: this is error-reporting
                // code, so it must not itself panic. The erroring scope can have
                // an invalid script_ref (cast_lib = u32::MAX sentinel), no lctx,
                // or an out-of-range handler id — in any of those cases just
                // report the error without a handler name.
                let current_handler_name = player
                    .movie
                    .cast_manager
                    .get_cast(current_scope.script_ref.cast_lib as u32)
                    .ok()
                    .and_then(|cast_lib| cast_lib.lctx.as_ref())
                    .and_then(|lctx| lctx.names.get(current_scope.handler_name_id as usize))
                    .cloned();

                OnScriptErrorCallbackData {
                    message: err.message.to_owned(),
                    script_member_ref: Some(current_scope.script_ref.to_js()),
                    handler_name: current_handler_name,
                    is_paused,
                }
                .into()
            } else {
                OnScriptErrorCallbackData {
                    message: err.message.to_owned(),
                    script_member_ref: None,
                    handler_name: None,
                    is_paused,
                }
                .into()
            };

        data.to_js_object()
    }

    pub fn dispatch_script_error(player: &DirPlayer, err: &ScriptError) {
        let _ = onScriptError(Self::script_error_data(player, err));
    }

    pub fn dispatch_script_error_data(data: js_sys::Object) -> Result<(), JsValue> {
        onScriptError(data)
    }

    pub fn dispatch_script_error_data_owned(
        owner_key: &str,
        data: js_sys::Object,
    ) -> Result<(), JsValue> {
        onScriptErrorOwned(owner_key, data)
    }

    /// The current breakpoint list, in the shape the JS bridge expects.
    ///
    /// Shared by the push (`dispatch_breakpoint_list_changed`) and the pull
    /// (`get_breakpoints`) so the two can never drift.
    pub fn get_breakpoint_list(player: &DirPlayer) -> Vec<js_sys::Object> {
        player
            .breakpoint_manager
            .breakpoints
            .iter()
            .map(|x| {
                let breakpoint = JsBridgeBreakpoint {
                    script_name: x.script_name.to_owned(),
                    handler_name: x.handler_name.to_owned(),
                    bytecode_index: x.bytecode_index,
                };
                let breakpoint_js: js_sys::Map = breakpoint.into();
                breakpoint_js.to_js_object()
            })
            .collect()
    }

    pub fn dispatch_breakpoint_list_changed() {
        crate::player::spawn_player_local(async move {
            // Deferred task — the player may be gone by the time it runs.
            if unsafe { PLAYER_OPT.is_none() } { return; }
            let player = unsafe { crate::player::player_ref() };
            onBreakpointListChanged(Self::get_breakpoint_list(player));
        });
    }

    pub fn dispatch_script_error_cleared() {
        onScriptErrorCleared();
    }

    pub fn dispatch_external_event(event: &str) {
        onExternalEvent(event);
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl JsApi {
    fn native_host_event_overflow(capacity: usize) -> JsValue {
        // JsValue has no native string payload. Keep the Result error signal
        // for callers while avoiding wasm-bindgen's native panic path; the
        // ordered event has already been retained for retry by queue_host_event.
        log::error!("native host event mailbox overflow (capacity {})", capacity);
        JsValue::NULL
    }

    fn native_datum_type_name(datum: &Datum) -> &'static str {
        match datum {
            Datum::Int(_) | Datum::Float(_) => "number",
            Datum::String(_) | Datum::StringChunk(..) => "string",
            Datum::Void => "void", Datum::VarRef(_) => "var_ref", Datum::List(..) => "list",
            Datum::PropList(..) => "propList", Datum::Symbol(_) => "symbol", Datum::CastLib(_) => "castLib",
            Datum::Stage => "stage", Datum::ScriptRef(_) => "scriptRef", Datum::ScriptInstanceRef(_) => "scriptInstance",
            Datum::CastMember(_) => "castMember", Datum::SpriteRef(_) => "spriteRef", Datum::Rect(..) => "Rect",
            Datum::Point(..) => "Point", Datum::SoundChannel(_) => "soundChannel", Datum::CursorRef(_) => "cursorRef",
            Datum::TimeoutRef(_) => "timeout", Datum::TimeoutFactory => "timeoutFactory", Datum::TimeoutInstance(_) => "timeoutInstance",
            Datum::ColorRef(_) => "colorRef", Datum::BitmapRef(_) => "bitmapRef", Datum::PaletteRef(_) => "paletteRef",
            Datum::SoundRef(_) => "soundRef", Datum::Xtra(_) => "xtra", Datum::XtraInstance(..) => "xtraInstance",
            Datum::Matte(_) => "matte", Datum::PlayerRef => "playerRef", Datum::MovieRef => "movieRef", Datum::MouseRef => "mouseRef",
            Datum::XmlRef(_) => "xmlRef", Datum::DateRef(_) => "dateRef", Datum::MathRef(_) => "mathRef", Datum::Vector(_) => "vector",
            Datum::Media(_) => "media", Datum::Null => "null", Datum::JavaScript(_) => "javascript", Datum::FlashObjectRef(_) => "flashObjectRef",
            Datum::Shockwave3dObjectRef(_) => "shockwave3dObjectRef", Datum::Transform3d(_) => "transform3d",
            Datum::HavokObjectRef(_) => "havokObjectRef", Datum::PhysXObjectRef(_) => "physXObjectRef",
            Datum::VectorVertexRef(..) => "vectorVertexRef", Datum::JsObjectRef(_) => "jsObjectRef",
        }
    }

    fn native_notification_name(kind: &PlayerNotificationKind) -> &'static str {
        match kind {
            PlayerNotificationKind::ScoreChanged => "ScoreChanged",
            PlayerNotificationKind::ChannelChanged(_) => "ChannelChanged",
            PlayerNotificationKind::ChannelNameChanged(_) => "ChannelNameChanged",
            PlayerNotificationKind::ChannelNamesChanged => "ChannelNamesChanged",
            PlayerNotificationKind::CastMemberChanged(_) => "CastMemberChanged",
            PlayerNotificationKind::CastMemberListChanged(_) => "CastMemberListChanged",
            PlayerNotificationKind::CastMemberNameChanged(_) => "CastMemberNameChanged",
            PlayerNotificationKind::DatumSnapshot(_) => "DatumSnapshot",
            PlayerNotificationKind::ScriptInstanceSnapshot(_) => "ScriptInstanceSnapshot",
            PlayerNotificationKind::Host(_) => "Host",
            PlayerNotificationKind::HostBackpressure(_) => "HostBackpressure",
        }
    }

    fn native_bounded_string(value: &str) -> String {
        const MAX_NATIVE_STRING: usize = 64 * 1024;
        const TRUNCATION_MARKER: &str = "<truncated>";
        if value.len() <= MAX_NATIVE_STRING {
            return value.to_owned();
        }
        let content_limit = MAX_NATIVE_STRING.saturating_sub(TRUNCATION_MARKER.len());
        let end = value
            .char_indices()
            .take_while(|(index, character)| {
                *index + character.len_utf8() <= content_limit
            })
            .map(|(index, character)| index + character.len_utf8())
            .last()
            .unwrap_or(0);
        let mut truncated = value[..end].to_owned();
        truncated.push_str(TRUNCATION_MARKER);
        truncated
    }

    fn native_datum_value(
        player: &DirPlayer,
        symbols: &SymbolTable,
        datum_ref: &DatumRef,
        depth: u8,
        active: &mut HashSet<DatumId>,
        budget: &mut usize,
    ) -> Result<NativeDatumValue, String> {
        let datum = match datum_ref {
            DatumRef::Void => &Datum::Void,
            _ => player
                .allocator
                .try_get_datum(datum_ref)
                .ok_or_else(|| format!("foreign or stale datum reference {datum_ref}"))?,
        };
        let datum_id = (!matches!(datum_ref, DatumRef::Void)).then(|| datum_ref.unwrap());
        if let Some(id) = datum_id {
            if !active.insert(id) {
                return Ok(NativeDatumValue::Opaque("<cycle>".to_owned()));
            }
        }
        if *budget == 0 {
            if let Some(id) = datum_id {
                active.remove(&id);
            }
            return Ok(NativeDatumValue::Opaque("<node-limit>".to_owned()));
        }
        *budget -= 1;
        let result = if depth > 20 {
            Ok(NativeDatumValue::Opaque("<depth-limit>".to_owned()))
        } else {
            match datum {
                Datum::Int(v) => Ok(NativeDatumValue::Int(*v)),
                Datum::Float(v) => Ok(NativeDatumValue::Float(*v)),
                Datum::String(v) => Ok(NativeDatumValue::String(Self::native_bounded_string(v))),
                Datum::Symbol(v) => symbols
                    .display(v)
                    .map(|value| NativeDatumValue::Symbol(value.to_owned()))
                    .map_err(|_| "foreign symbol in native snapshot".to_owned()),
                Datum::Void => Ok(NativeDatumValue::Null),
                Datum::Null => Ok(NativeDatumValue::Null),
                Datum::List(_, values, _) => {
                    let mut converted = Vec::new();
                    for (index, value) in values.iter().enumerate() {
                        if *budget == 0 {
                            converted.push(NativeDatumValue::Opaque(
                                "<omitted-tail>".to_owned(),
                            ));
                            break;
                        }
                        let converted_value = Self::native_datum_value(
                            player,
                            symbols,
                            value,
                            depth + 1,
                            active,
                            budget,
                        )?;
                        let exhausted = *budget == 0;
                        converted.push(converted_value);
                        if exhausted && index + 1 < values.len() {
                            converted.push(NativeDatumValue::Opaque(
                                "<omitted-tail>".to_owned(),
                            ));
                            break;
                        }
                    }
                    Ok(NativeDatumValue::Array(converted))
                }
                Datum::PropList(values, sorted) => {
                    let mut converted = Vec::new();
                    for (index, (key, value)) in values.iter().enumerate() {
                        if *budget == 0 {
                            converted.push((
                                "<omitted-tail>".to_owned(),
                                NativeDatumValue::Opaque("<omitted-tail>".to_owned()),
                            ));
                            break;
                        }
                        let key_value = Self::native_datum_value(
                            player,
                            symbols,
                            key,
                            depth + 1,
                            active,
                            budget,
                        )?;
                        if *budget == 0 {
                            converted.push((
                                "<omitted-tail>".to_owned(),
                                NativeDatumValue::Opaque("<omitted-tail>".to_owned()),
                            ));
                            break;
                        }
                        let key = match key_value {
                            NativeDatumValue::String(value) | NativeDatumValue::Symbol(value) => value,
                            NativeDatumValue::Int(value) => value.to_string(),
                            other => Self::native_value_debug(&other),
                        };
                        let converted_value = Self::native_datum_value(
                            player,
                            symbols,
                            value,
                            depth + 1,
                            active,
                            budget,
                        )?;
                        let exhausted = *budget == 0;
                        converted.push((key, converted_value));
                        if exhausted && index + 1 < values.len() {
                            converted.push((
                                "<omitted-tail>".to_owned(),
                                NativeDatumValue::Opaque("<omitted-tail>".to_owned()),
                            ));
                            break;
                        }
                    }
                    Ok(NativeDatumValue::PropList(converted, *sorted))
                }
                Datum::StringChunk(source, _, value) => {
                    if let crate::director::lingo::datum::StringChunkSource::Datum(source) = source {
                        let _ = Self::native_datum_value(player, symbols, source, depth + 1, active, budget)?;
                    }
                    Ok(NativeDatumValue::String(Self::native_bounded_string(value)))
                }
                Datum::VarRef(crate::director::lingo::datum::VarRef::ScriptInstance(instance)) => {
                    player.allocator.get_script_instance_opt(instance).ok_or_else(|| {
                        format!("foreign or stale script instance {}", instance.id())
                    })?;
                    Ok(NativeDatumValue::Reference {
                        kind: "scriptInstance".to_owned(),
                        id: instance.id().to_string(),
                    })
                }
                Datum::VarRef(_) => Ok(NativeDatumValue::Opaque("<var-ref>".to_owned())),
                Datum::TimeoutInstance(timeout) => {
                    let _ = Self::native_datum_value(player, symbols, &timeout.callback, depth + 1, active, budget)?;
                    let _ = Self::native_datum_value(player, symbols, &timeout.target, depth + 1, active, budget)?;
                    if let Some(script_instance) = &timeout.script_instance {
                        let _ = Self::native_datum_value(player, symbols, script_instance, depth + 1, active, budget)?;
                    }
                    Ok(NativeDatumValue::Opaque("<timeout-instance>".to_owned()))
                }
                Datum::ScriptInstanceRef(instance) => {
                    player.allocator.get_script_instance_opt(instance).ok_or_else(|| {
                        format!("foreign or stale script instance {}", instance.id())
                    })?;
                    Ok(NativeDatumValue::Reference {
                        kind: "scriptInstance".to_owned(),
                        id: instance.id().to_string(),
                    })
                }
                Datum::ScriptRef(reference) | Datum::CastMember(reference) => {
                    Ok(NativeDatumValue::Reference {
                        kind: "castMember".to_owned(),
                        id: format!("{}:{}", reference.cast_lib, reference.cast_member),
                    })
                }
                datum => Ok(NativeDatumValue::Opaque(format!(
                    "<{}>",
                    Self::native_datum_type_name(datum)
                ))),
            }
        };
        if let Some(id) = datum_id {
            active.remove(&id);
        }
        result
    }

    fn native_value_debug(value: &NativeDatumValue) -> String {
        const MAX_NATIVE_STRING: usize = 64 * 1024;
        const TRUNCATION_MARKER: &str = "<truncated>";

        fn append_bounded(output: &mut String, value: &str, truncated: &mut bool) {
            if *truncated {
                return;
            }
            let content_limit = MAX_NATIVE_STRING.saturating_sub(TRUNCATION_MARKER.len());
            let available = content_limit.saturating_sub(output.len());
            if value.len() <= available {
                output.push_str(value);
                return;
            }
            let end = value
                .char_indices()
                .take_while(|(index, character)| *index + character.len_utf8() <= available)
                .map(|(index, character)| index + character.len_utf8())
                .last()
                .unwrap_or(0);
            output.push_str(&value[..end]);
            *truncated = true;
        }

        fn append_value(value: &NativeDatumValue, output: &mut String, truncated: &mut bool) {
            if *truncated {
                return;
            }
            match value {
                NativeDatumValue::Null => append_bounded(output, "void", truncated),
                NativeDatumValue::Bool(value) => append_bounded(output, &value.to_string(), truncated),
                NativeDatumValue::Int(value) => append_bounded(output, &value.to_string(), truncated),
                NativeDatumValue::Float(value) => append_bounded(output, &value.to_string(), truncated),
                NativeDatumValue::String(value) => {
                    append_bounded(output, "\"", truncated);
                    append_bounded(output, value, truncated);
                    append_bounded(output, "\"", truncated);
                }
                NativeDatumValue::Symbol(value) => {
                    append_bounded(output, "#", truncated);
                    append_bounded(output, value, truncated);
                }
                NativeDatumValue::Array(values) => {
                    append_bounded(output, "[", truncated);
                    for (index, value) in values.iter().enumerate() {
                        if index > 0 {
                            append_bounded(output, ", ", truncated);
                        }
                        append_value(value, output, truncated);
                        if *truncated {
                            break;
                        }
                    }
                    append_bounded(output, "]", truncated);
                }
                NativeDatumValue::PropList(values, _) => {
                    append_bounded(output, "[", truncated);
                    for (index, (key, value)) in values.iter().enumerate() {
                        if index > 0 {
                            append_bounded(output, ", ", truncated);
                        }
                        append_bounded(output, key, truncated);
                        append_bounded(output, ": ", truncated);
                        append_value(value, output, truncated);
                        if *truncated {
                            break;
                        }
                    }
                    append_bounded(output, "]", truncated);
                }
                NativeDatumValue::Reference { kind, id } => {
                    append_bounded(output, kind, truncated);
                    append_bounded(output, "(", truncated);
                    append_bounded(output, id, truncated);
                    append_bounded(output, ")", truncated);
                }
                NativeDatumValue::Opaque(value) => append_bounded(output, value, truncated),
            }
        }

        let mut rendered = String::new();
        let mut truncated = false;
        append_value(value, &mut rendered, &mut truncated);
        if truncated {
            rendered.push_str(TRUNCATION_MARKER);
        }
        rendered
    }

    fn native_datum_snapshot_with_budget(
        player: &DirPlayer,
        symbols: &SymbolTable,
        datum_ref: &DatumRef,
        budget: &mut usize,
    ) -> Result<NativeDatumSnapshot, String> {
        let value = Self::native_datum_value(
            player,
            symbols,
            datum_ref,
            0,
            &mut HashSet::new(),
            budget,
        )?;
        let datum = match datum_ref {
            DatumRef::Void => &Datum::Void,
            _ => player
                .allocator
                .try_get_datum(datum_ref)
                .ok_or_else(|| format!("foreign or stale datum reference {datum_ref}"))?,
        };
        Ok(NativeDatumSnapshot {
            datum_id: (!matches!(datum_ref, DatumRef::Void)).then(|| datum_ref.unwrap()).unwrap_or(0),
            type_name: Self::native_datum_type_name(datum).to_owned(),
            debug_description: Self::native_bounded_string(&Self::native_value_debug(&value)),
            value,
        })
    }

    fn native_datum_snapshot(
        player: &DirPlayer,
        symbols: &SymbolTable,
        datum_ref: &DatumRef,
    ) -> Result<NativeDatumSnapshot, String> {
        let mut budget = 4096;
        Self::native_datum_snapshot_with_budget(player, symbols, datum_ref, &mut budget)
    }
    fn native_score_snapshot_for_score(player: &DirPlayer, score: &Score) -> NativeScoreSnapshot {
        const MAX_NATIVE_SCORE_SPANS: usize = 4096;
        let mut truncated = score.sprite_spans.len() > MAX_NATIVE_SCORE_SPANS;
        let spans = score.sprite_spans.iter().take(MAX_NATIVE_SCORE_SPANS).map(|span| {
            let mut span_truncated = span.scripts.len() > MAX_NATIVE_SCORE_SPANS;
            truncated |= span_truncated;
            let member_ref = span.scripts.first().map(|s| (s.cast_lib, s.cast_member)).unwrap_or((0,0));
            let member_name = player.movie.cast_manager.find_member_by_ref(&CastMemberRef { cast_lib: member_ref.0 as i32, cast_member: member_ref.1 as i32 }).map(|m| Self::native_bounded_string(&m.name)).unwrap_or_default();
            NativeScoreSpan { channel_number: span.channel_number, start_frame: span.start_frame, end_frame: span.end_frame, member_ref, member_name,
                behavior_references: span.scripts.iter().take(MAX_NATIVE_SCORE_SPANS).map(|s| {
                    let mut parameter_debug = s.parameter.iter().take(MAX_NATIVE_SCORE_SPANS)
                        .map(|parameter| Self::native_bounded_string(&format!("{:?}", parameter)))
                        .collect::<Vec<_>>();
                    if s.parameter.len() > MAX_NATIVE_SCORE_SPANS {
                        parameter_debug.push("<omitted-tail>".to_owned());
                        span_truncated = true;
                        truncated = true;
                    }
                    NativeBehaviorReference { cast_lib:s.cast_lib, cast_member:s.cast_member, parameter_debug }
                }).collect(),
                truncated: span_truncated,
            }
        }).collect();
        NativeScoreSnapshot { channel_count: score.channels.len() as u16, frame_count: score.frame_count.unwrap_or(1), spans, truncated }
    }
    fn native_score_snapshot(player: &DirPlayer) -> NativeScoreSnapshot {
        Self::native_score_snapshot_for_score(player, &player.movie.score)
    }
    fn native_channel_snapshot(player: &DirPlayer, channel: i16) -> Result<NativeChannelSnapshot, String> {
        let channel_data = channel.try_into().ok().and_then(|i: usize| player.movie.score.channels.get(i));
        let Some(channel_data) = channel_data else { return Ok(NativeChannelSnapshot { channel, display_name:String::new(), member_ref:None, script_instance_ids:Vec::new(), width:0,height:0,loc_h:0,loc_v:0,visible:false,puppet:false,ink:0,blend:0,rotation:0.0,skew:0.0,flip_h:false,flip_v:false,color:String::new(),bg_color:String::new() }); };
        let display_name = if !channel_data.name.is_empty() { channel_data.name.clone() } else if !channel_data.sprite.name.is_empty() { channel_data.sprite.name.clone() } else { channel_data.sprite.member.as_ref().and_then(|r| player.movie.cast_manager.find_member_by_ref(r)).map(|m|m.name.clone()).unwrap_or_default() };
        let script_instance_ids = channel_data.sprite.script_instance_list.iter().map(|reference| {
            player.allocator.get_script_instance_opt(reference)
                .ok_or_else(|| format!("foreign or stale channel script instance {}", reference.id()))
                .map(|_| reference.id())
        }).collect::<Result<Vec<_>, _>>()?;
        Ok(NativeChannelSnapshot { channel, display_name:Self::native_bounded_string(&display_name), member_ref:channel_data.sprite.member.as_ref().map(|r|(r.cast_lib,r.cast_member)), script_instance_ids, width:channel_data.sprite.width,height:channel_data.sprite.height,loc_h:channel_data.sprite.loc_h,loc_v:channel_data.sprite.loc_v,visible:channel_data.sprite.visible,puppet:channel_data.sprite.puppet,ink:channel_data.sprite.ink,blend:channel_data.sprite.blend,rotation:channel_data.sprite.rotation,skew:channel_data.sprite.skew,flip_h:channel_data.sprite.flip_h,flip_v:channel_data.sprite.flip_v,color:channel_data.sprite.color.to_string(),bg_color:channel_data.sprite.bg_color.to_string() })
    }
    fn native_script_type_name(script_type: ScriptType) -> &'static str {
        match script_type {
            ScriptType::Movie => "movie",
            ScriptType::Parent => "parent",
            ScriptType::Score => "score",
            _ => "unknown",
        }
    }

    fn native_member_script_snapshot(
        player: &DirPlayer,
        symbols: &SymbolTable,
        member_ref: CastMemberRef,
        member: &ScriptMember,
    ) -> Result<NativeDatumValue, String> {
        use crate::director::lingo::opcode::OpCode;

        const MAX_ITEMS: usize = 4096;
        const MAX_HANDLERS: usize = 256;
        const MAX_BYTECODE: usize = 256;
        const MAX_SOURCE_BYTES: usize = 64 * 1024;
        const MAX_NATIVE_STRING: usize = 64 * 1024;
        let cast = player
            .movie
            .cast_manager
            .get_cast(member_ref.cast_lib as u32)
            .map_err(|error| error.message.clone())?;
        let lctx = cast
            .lctx
            .as_ref()
            .ok_or_else(|| format!("missing script context for cast {}", member_ref.cast_lib))?;
        let script = cast
            .get_script_for_member(member_ref.cast_member as u32)
            .ok_or_else(|| format!("missing registered script {}:{}", member_ref.cast_lib, member_ref.cast_member))?;
        let multiplier = get_variable_multiplier(cast.capital_x, cast.dir_version);
        if multiplier == 0 {
            return Err(format!("invalid variable multiplier for cast {}", member_ref.cast_lib));
        }
        let source_bytes = script
            .chunk
            .literals
            .iter()
            .map(|literal| match literal {
                Datum::String(value) | Datum::StringChunk(_, _, value) => value.len(),
                Datum::JavaScript(value) => value.len(),
                _ => 0,
            })
            .try_fold(0usize, |total, size| total.checked_add(size))
            .unwrap_or(MAX_SOURCE_BYTES.saturating_add(1));
        let context_name_bytes = lctx
            .names
            .iter()
            .map(|name| name.len())
            .try_fold(0usize, |total, size| total.checked_add(size))
            .unwrap_or(MAX_SOURCE_BYTES.saturating_add(1));
        let oversized_context_name = context_name_bytes > MAX_SOURCE_BYTES
            || lctx.names.iter().any(|name| name.len() > MAX_NATIVE_STRING);
        let mut budget = MAX_ITEMS;
        let mut fields = vec![
            ("name".to_owned(), NativeDatumValue::String(Self::native_bounded_string(&script.name))),
            ("script_type".to_owned(), NativeDatumValue::String(Self::native_script_type_name(member.script_type).to_owned())),
            ("script_syntax".to_owned(), NativeDatumValue::String(
                if script.chunk.literals.iter().any(|literal| matches!(literal, Datum::JavaScript(_))) { "javascript" } else { "lingo" }.to_owned(),
            )),
        ];
        let mut handlers = Vec::new();
        for (handler_index, handler) in script.chunk.handlers.iter().take(MAX_HANDLERS).enumerate() {
            if budget == 0 { break; }
            budget -= 1;
            let validate_instruction = |instruction: &crate::director::chunks::handler::Bytecode| -> Result<(), String> {
                let needs_name = matches!(instruction.opcode,
                    OpCode::ObjCall | OpCode::ExtCall | OpCode::GetObjProp | OpCode::SetObjProp |
                    OpCode::PushSymb | OpCode::GetProp | OpCode::GetChainedProp | OpCode::GetGlobal | OpCode::SetGlobal);
                if needs_name {
                    let name_index = usize::try_from(instruction.obj).map_err(|_| format!("invalid name operand in script handler {handler_index}"))?;
                    if lctx.names.get(name_index).is_none() {
                        return Err(format!("invalid name operand {} in script handler {handler_index}", instruction.obj));
                    }
                }
                match instruction.opcode {
                    OpCode::Jmp | OpCode::JmpIfZ => {
                        let target = (instruction.pos as i128)
                            .checked_add(instruction.obj as i128)
                            .and_then(|target| usize::try_from(target).ok())
                            .ok_or_else(|| format!("invalid jump target in script handler {handler_index}"))?;
                        let _ = target;
                    }
                    OpCode::EndRepeat => {
                        let target = (instruction.pos as i128)
                            .checked_sub(instruction.obj as i128)
                            .and_then(|target| usize::try_from(target).ok())
                            .ok_or_else(|| format!("invalid repeat target in script handler {handler_index}"))?;
                        let _ = target;
                    }
                    _ => {}
                }
                Ok(())
            };
            if handler.bytecode_array.len() <= MAX_BYTECODE {
                for instruction in &handler.bytecode_array {
                    validate_instruction(instruction)?;
                }
            }
            let handler_name = lctx
                .names
                .get(handler.name_id as usize)
                .map(|name| Self::native_bounded_string(name))
                .unwrap_or_else(|| "<anonymous>".to_owned());
            let mut args = Vec::new();
            for name_id in handler.argument_name_ids.iter().take(MAX_ITEMS) {
                let name = lctx.names.get(*name_id as usize).ok_or_else(|| {
                    format!("invalid argument name {} in script handler {handler_index}", name_id)
                })?;
                if budget == 0 { break; }
                budget -= 1;
                args.push(NativeDatumValue::String(Self::native_bounded_string(name)));
            }
            if handler.argument_name_ids.len() > args.len() {
                args.push(NativeDatumValue::Opaque("<omitted-tail>".to_owned()));
            }
            let mut bytecode = Vec::new();
            for instruction in handler.bytecode_array.iter().take(MAX_BYTECODE) {
                if budget == 0 { break; }
                budget -= 1;
                validate_instruction(instruction)?;
                let text = if oversized_context_name {
                    "<truncated>".to_owned()
                } else if matches!(instruction.opcode, OpCode::Jmp | OpCode::JmpIfZ) {
                    let target = (instruction.pos as i128)
                        .checked_add(instruction.obj as i128)
                        .and_then(|target| usize::try_from(target).ok())
                        .ok_or_else(|| format!("invalid jump target in script handler {handler_index}"))?;
                    format!("[{}] {} [{}]", instruction.pos, crate::director::lingo::constants::get_opcode_name(instruction.opcode), target)
                } else if matches!(instruction.opcode, OpCode::EndRepeat) {
                    let target = (instruction.pos as i128)
                        .checked_sub(instruction.obj as i128)
                        .and_then(|target| usize::try_from(target).ok())
                        .ok_or_else(|| format!("invalid repeat target in script handler {handler_index}"))?;
                    format!("[{}] {} [{}]", instruction.pos, crate::director::lingo::constants::get_opcode_name(instruction.opcode), target)
                } else {
                    instruction.to_bytecode_text(lctx, handler, multiplier)
                };
                bytecode.push(NativeDatumValue::PropList(vec![
                    ("pos".to_owned(), NativeDatumValue::Int(instruction.pos as i32)),
                    ("text".to_owned(), NativeDatumValue::String(Self::native_bounded_string(&text))),
                ], false));
            }
            if handler.bytecode_array.len() > MAX_BYTECODE || budget == 0 {
                bytecode.push(NativeDatumValue::Opaque("<omitted-tail>".to_owned()));
            }

            let mut lingo = Vec::new();
            let mut bytecode_to_line = Vec::new();
            if handler.bytecode_array.len() <= MAX_BYTECODE
                && source_bytes <= MAX_SOURCE_BYTES
                && !oversized_context_name
            {
                let decompiled = decompiler::decompile_handler(handler, &script.chunk, lctx, cast.dir_version, multiplier, symbols)
                    .map_err(|error| format!("script handler {handler_index} decompile failed: {}", error.message))?;
                for line in decompiled.lines.iter().take(MAX_ITEMS) {
                    if budget == 0 { break; }
                    budget -= 1;
                    let mut spans = Vec::new();
                    for span in line.spans.iter().take(MAX_ITEMS) {
                        if budget == 0 { break; }
                        budget -= 1;
                        spans.push(NativeDatumValue::PropList(vec![
                            ("text".to_owned(), NativeDatumValue::String(Self::native_bounded_string(&span.text))),
                            ("type".to_owned(), NativeDatumValue::String(span.token_type.as_str().to_owned())),
                        ], false));
                    }
                    if line.spans.len() > spans.len() {
                        spans.push(NativeDatumValue::Opaque("<omitted-tail>".to_owned()));
                    }
                    let mut bytecode_indices = Vec::new();
                    for index in line.bytecode_indices.iter().take(MAX_ITEMS) {
                        if budget == 0 { break; }
                        budget -= 1;
                        bytecode_indices.push(NativeDatumValue::Int(*index as i32));
                    }
                    if line.bytecode_indices.len() > bytecode_indices.len() {
                        bytecode_indices.push(NativeDatumValue::Opaque("<omitted-tail>".to_owned()));
                    }
                    lingo.push(NativeDatumValue::PropList(vec![
                        ("text".to_owned(), NativeDatumValue::String(Self::native_bounded_string(&line.text))),
                        ("indent".to_owned(), NativeDatumValue::Int(line.indent as i32)),
                        ("bytecodeIndices".to_owned(), NativeDatumValue::Array(bytecode_indices)),
                        ("spans".to_owned(), NativeDatumValue::Array(spans)),
                    ], false));
                }
                if decompiled.lines.len() > lingo.len() { lingo.push(NativeDatumValue::Opaque("<omitted-tail>".to_owned())); }
                for (bytecode, line) in decompiled.bytecode_to_line.iter().take(MAX_ITEMS) {
                    if budget == 0 { break; }
                    budget -= 1;
                    bytecode_to_line.push((bytecode.to_string(), NativeDatumValue::Int(*line as i32)));
                }
                if decompiled.bytecode_to_line.len() > bytecode_to_line.len() {
                    bytecode_to_line.push(("<omitted-tail>".to_owned(), NativeDatumValue::Opaque("<omitted-tail>".to_owned())));
                }
            } else {
                lingo.push(NativeDatumValue::Opaque("<omitted-tail>".to_owned()));
            }
            handlers.push(NativeDatumValue::PropList(vec![
                ("name".to_owned(), NativeDatumValue::String(Self::native_bounded_string(&handler_name))),
                ("args".to_owned(), NativeDatumValue::Array(args)),
                ("bytecode".to_owned(), NativeDatumValue::Array(bytecode)),
                ("lingo".to_owned(), NativeDatumValue::Array(lingo)),
                ("bytecodeToLine".to_owned(), NativeDatumValue::PropList(bytecode_to_line, false)),
            ], false));
        }
        if script.chunk.handlers.len() > MAX_HANDLERS || budget == 0 {
            handlers.push(NativeDatumValue::Opaque("<omitted-tail>".to_owned()));
        }
        fields.push(("handlers".to_owned(), NativeDatumValue::Array(handlers)));
        Ok(NativeDatumValue::PropList(fields, false))
    }

    fn native_member_snapshot(
        player: &DirPlayer,
        symbols: &SymbolTable,
        member_ref: CastMemberRef,
    ) -> Result<NativeMemberSnapshot, String> {
        let member = player
            .movie
            .cast_manager
            .find_member_by_ref(&member_ref)
            .ok_or_else(|| {
                format!(
                    "missing cast member {}:{} in native snapshot",
                    member_ref.cast_lib, member_ref.cast_member
                )
            })?;
        let mut fields = Vec::new();
        {
            fields.push(("type".to_owned(), NativeDatumValue::String(member.member_type.type_string().to_owned())));
            fields.push(("name".to_owned(), NativeDatumValue::String(Self::native_bounded_string(&member.name))));
            fields.push(("number".to_owned(), NativeDatumValue::Int(member.number as i32)));
            fields.push(("color".to_owned(), NativeDatumValue::String(member.color.to_string())));
            fields.push(("bgColor".to_owned(), NativeDatumValue::String(member.bg_color.to_string())));
            fields.push(("regX".to_owned(), NativeDatumValue::Int(member.reg_point.0)));
            fields.push(("regY".to_owned(), NativeDatumValue::Int(member.reg_point.1)));
            match &member.member_type {
                CastMemberType::Field(data) => {
                    fields.push(("text".to_owned(), NativeDatumValue::String(Self::native_bounded_string(&data.text))));
                    fields.push(("width".to_owned(), NativeDatumValue::Int(data.width as i32)));
                    fields.push(("height".to_owned(), NativeDatumValue::Int(data.height as i32)));
                    fields.push(("editable".to_owned(), NativeDatumValue::Bool(data.editable)));
                    fields.push(("wordWrap".to_owned(), NativeDatumValue::Bool(data.word_wrap)));
                }
                CastMemberType::Text(data) => {
                    fields.push(("text".to_owned(), NativeDatumValue::String(Self::native_bounded_string(&data.text))));
                    fields.push(("htmlSource".to_owned(), NativeDatumValue::String(Self::native_bounded_string(&data.html_source))));
                    fields.push(("alignment".to_owned(), NativeDatumValue::String(Self::native_bounded_string(data.alignment.as_str()))));
                    fields.push(("boxType".to_owned(), NativeDatumValue::String(Self::native_bounded_string(&data.box_type.to_string()))));
                    fields.push(("width".to_owned(), NativeDatumValue::Int(data.width as i32)));
                    fields.push(("height".to_owned(), NativeDatumValue::Int(data.height as i32)));
                    fields.push(("wordWrap".to_owned(), NativeDatumValue::Bool(data.word_wrap)));
                    fields.push(("antiAlias".to_owned(), NativeDatumValue::Bool(data.anti_alias)));
                    fields.push(("font".to_owned(), NativeDatumValue::String(Self::native_bounded_string(&data.font))));
                    let mut font_style = data.font_style.iter().take(4096).map(|style| {
                        NativeDatumValue::String(Self::native_bounded_string(&style.to_string()))
                    }).collect::<Vec<_>>();
                    if data.font_style.len() > 4096 {
                        font_style.push(NativeDatumValue::Opaque("<omitted-tail>".to_owned()));
                    }
                    fields.push(("fontStyle".to_owned(), NativeDatumValue::Array(font_style)));
                    fields.push(("fixedLineSpace".to_owned(), NativeDatumValue::Float(data.fixed_line_space as f64)));
                    fields.push(("topSpacing".to_owned(), NativeDatumValue::Float(data.top_spacing as f64)));
                    fields.push(("bottomSpacing".to_owned(), NativeDatumValue::Float(data.bottom_spacing as f64)));
                    let mut html_styled_spans = data.html_styled_spans.iter().take(4096).map(|span| {
                        NativeDatumValue::PropList(vec![
                            ("text".to_owned(), NativeDatumValue::String(Self::native_bounded_string(&span.text))),
                            ("fontFace".to_owned(), NativeDatumValue::String(Self::native_bounded_string(&span.style.font_face.clone().unwrap_or_default()))),
                            ("fontSize".to_owned(), NativeDatumValue::Float(span.style.font_size.unwrap_or_default() as f64)),
                            ("bold".to_owned(), NativeDatumValue::Bool(span.style.bold)),
                            ("italic".to_owned(), NativeDatumValue::Bool(span.style.italic)),
                            ("underline".to_owned(), NativeDatumValue::Bool(span.style.underline)),
                            ("color".to_owned(), NativeDatumValue::Int(span.style.color.unwrap_or_default() as i32)),
                        ], false)
                    }).collect::<Vec<_>>();
                    if data.html_styled_spans.len() > 4096 {
                        html_styled_spans.push(NativeDatumValue::Opaque("<omitted-tail>".to_owned()));
                    }
                    fields.push(("htmlStyledSpans".to_owned(), NativeDatumValue::Array(html_styled_spans)));
                }
                CastMemberType::Script(data) => {
                    fields.push((
                        "script".to_owned(),
                        Self::native_member_script_snapshot(player, symbols, member_ref.clone(), data)?,
                    ));
                }
                CastMemberType::Shape(data) => {
                    let info = &data.shape_info;
                    let shape_type = match info.shape_type {
                        crate::director::enums::ShapeType::Rect => "rect",
                        crate::director::enums::ShapeType::OvalRect => "roundRect",
                        crate::director::enums::ShapeType::Oval => "oval",
                        crate::director::enums::ShapeType::Line => "line",
                        crate::director::enums::ShapeType::Unknown => "rect",
                    };
                    fields.extend([
                        ("shapeType".to_owned(), NativeDatumValue::String(shape_type.to_owned())),
                        ("width".to_owned(), NativeDatumValue::Int(info.width() as i32)),
                        ("height".to_owned(), NativeDatumValue::Int(info.height() as i32)),
                        ("rectLeft".to_owned(), NativeDatumValue::Int(info.rect_left as i32)),
                        ("rectTop".to_owned(), NativeDatumValue::Int(info.rect_top as i32)),
                        ("rectRight".to_owned(), NativeDatumValue::Int(info.rect_right as i32)),
                        ("rectBottom".to_owned(), NativeDatumValue::Int(info.rect_bottom as i32)),
                        ("pattern".to_owned(), NativeDatumValue::Int(info.pattern as i32)),
                        ("foreColor".to_owned(), NativeDatumValue::Int(info.fore_color as i32)),
                        ("backColor".to_owned(), NativeDatumValue::Int(info.back_color as i32)),
                        ("filled".to_owned(), NativeDatumValue::Bool(info.fill_type != 0)),
                        ("lineSize".to_owned(), NativeDatumValue::Int((info.line_thickness as i32 - 1).max(0))),
                        ("lineDirection".to_owned(), NativeDatumValue::Int(info.line_direction as i32)),
                    ]);
                }
                CastMemberType::Bitmap(data) => {
                    let bitmap = player.bitmap_manager.get_bitmap(data.image_ref).ok_or_else(|| {
                        format!("missing bitmap for cast member {}:{}", member_ref.cast_lib, member_ref.cast_member)
                    })?;
                    fields.push(("width".to_owned(), NativeDatumValue::Int(bitmap.width as i32)));
                    fields.push(("height".to_owned(), NativeDatumValue::Int(bitmap.height as i32)));
                    fields.push(("bitDepth".to_owned(), NativeDatumValue::Int(bitmap.bit_depth as i32)));
                    fields.push(("paletteRef".to_owned(), NativeDatumValue::String(Self::native_bounded_string(&format!("{:?}", bitmap.palette_ref)))));
                }
                CastMemberType::Sound(data) => {
                    fields.push(("sampleRate".to_owned(), NativeDatumValue::Int(data.info.sample_rate as i32)));
                    fields.push(("channels".to_owned(), NativeDatumValue::Int(data.info.channels as i32)));
                    fields.push(("bitsPerSample".to_owned(), NativeDatumValue::Int(data.info.sample_size as i32)));
                    fields.push(("sampleCount".to_owned(), NativeDatumValue::Int(data.info.sample_count as i32)));
                    fields.push(("duration".to_owned(), NativeDatumValue::Float(data.info.duration as f64)));
                    fields.push(("loop".to_owned(), NativeDatumValue::Bool(data.info.loop_enabled)));
                    fields.push(("codec".to_owned(), NativeDatumValue::String(data.sound.codec())));
                    fields.push(("dataSize".to_owned(), NativeDatumValue::Int(data.sound.data().len() as i32)));
                }
                CastMemberType::FilmLoop(data) => {
                    fields.push(("width".to_owned(), NativeDatumValue::Int(data.info.width as i32)));
                    fields.push(("height".to_owned(), NativeDatumValue::Int(data.info.height as i32)));
                    fields.push(("center".to_owned(), NativeDatumValue::Int(data.info.center as i32)));
                    fields.push(("regX".to_owned(), NativeDatumValue::Int(data.info.reg_point.0.into())));
                    fields.push(("regY".to_owned(), NativeDatumValue::Int(data.info.reg_point.1.into())));
                    let score = Self::native_score_snapshot_for_score(player, &data.score);
                    let spans = score.spans.into_iter().map(|span| {
                        let behaviors = NativeDatumValue::Array(span.behavior_references.into_iter().map(|behavior| {
                            NativeDatumValue::PropList(vec![
                                ("castLib".to_owned(), NativeDatumValue::Int(behavior.cast_lib as i32)),
                                ("castMember".to_owned(), NativeDatumValue::Int(behavior.cast_member as i32)),
                                ("parameterDebug".to_owned(), NativeDatumValue::Array(behavior.parameter_debug.into_iter().map(NativeDatumValue::String).collect())),
                            ], false)
                        }).collect());
                        NativeDatumValue::PropList(vec![
                            ("channelNumber".to_owned(), NativeDatumValue::Int(span.channel_number as i32)),
                            ("startFrame".to_owned(), NativeDatumValue::Int(span.start_frame as i32)),
                            ("endFrame".to_owned(), NativeDatumValue::Int(span.end_frame as i32)),
                            ("memberName".to_owned(), NativeDatumValue::String(Self::native_bounded_string(&span.member_name))),
                            ("behaviorReferences".to_owned(), behaviors),
                        ], false)
                    }).collect();
                    fields.push(("score".to_owned(), NativeDatumValue::PropList(vec![
                        ("channelCount".to_owned(), NativeDatumValue::Int(score.channel_count as i32)),
                        ("frameCount".to_owned(), NativeDatumValue::Int(score.frame_count as i32)),
                        ("behaviorReferences".to_owned(), NativeDatumValue::Array(spans)),
                    ], false)));
                }
                CastMemberType::Flash(data) => {
                    fields.push(("regX".to_owned(), NativeDatumValue::Int(data.reg_point.0.into())));
                    fields.push(("regY".to_owned(), NativeDatumValue::Int(data.reg_point.1.into())));
                    fields.push(("dataSize".to_owned(), NativeDatumValue::Int(data.data.len() as i32)));
                    if let Some(info) = &data.flash_info {
                        fields.extend([
                            ("flashRectLeft".to_owned(), NativeDatumValue::Int(info.flash_rect.0)),
                            ("flashRectTop".to_owned(), NativeDatumValue::Int(info.flash_rect.1)),
                            ("flashRectRight".to_owned(), NativeDatumValue::Int(info.flash_rect.2)),
                            ("flashRectBottom".to_owned(), NativeDatumValue::Int(info.flash_rect.3)),
                            ("width".to_owned(), NativeDatumValue::Int(info.flash_rect.2 - info.flash_rect.0)),
                            ("height".to_owned(), NativeDatumValue::Int(info.flash_rect.3 - info.flash_rect.1)),
                            ("directToStage".to_owned(), NativeDatumValue::Bool(info.direct_to_stage)),
                            ("imageEnabled".to_owned(), NativeDatumValue::Bool(info.image_enabled)),
                            ("soundEnabled".to_owned(), NativeDatumValue::Bool(info.sound_enabled)),
                            ("pausedAtStart".to_owned(), NativeDatumValue::Bool(info.paused_at_start)),
                            ("loop".to_owned(), NativeDatumValue::Bool(info.loop_enabled)),
                            ("isStatic".to_owned(), NativeDatumValue::Bool(info.is_static)),
                            ("preload".to_owned(), NativeDatumValue::Bool(info.preload)),
                            ("centerRegPoint".to_owned(), NativeDatumValue::Bool(info.center_reg_point)),
                            ("buttonsEnabled".to_owned(), NativeDatumValue::Bool(info.buttons_enabled)),
                            ("actionsEnabled".to_owned(), NativeDatumValue::Bool(info.actions_enabled)),
                            ("fixedRate".to_owned(), NativeDatumValue::Int(info.fixed_rate as i32)),
                            ("posterFrame".to_owned(), NativeDatumValue::Int(info.poster_frame as i32)),
                            ("bufferSize".to_owned(), NativeDatumValue::Int(info.buffer_size as i32)),
                            ("scale".to_owned(), NativeDatumValue::Float(info.scale as f64)),
                            ("viewScale".to_owned(), NativeDatumValue::Float(info.view_scale as f64)),
                            ("originH".to_owned(), NativeDatumValue::Float(info.origin_h as f64)),
                            ("originV".to_owned(), NativeDatumValue::Float(info.origin_v as f64)),
                            ("viewH".to_owned(), NativeDatumValue::Float(info.view_h as f64)),
                            ("viewV".to_owned(), NativeDatumValue::Float(info.view_v as f64)),
                            ("originMode".to_owned(), NativeDatumValue::String(Self::native_bounded_string(match info.origin_mode {
                                crate::director::enums::FlashOriginMode::Center => "center",
                                crate::director::enums::FlashOriginMode::TopLeft => "topLeft",
                                crate::director::enums::FlashOriginMode::Point => "point",
                            }))),
                            ("playbackMode".to_owned(), NativeDatumValue::String(Self::native_bounded_string(match info.playback_mode {
                                crate::director::enums::FlashPlaybackMode::Normal => "normal",
                                crate::director::enums::FlashPlaybackMode::Fixed => "fixed",
                                crate::director::enums::FlashPlaybackMode::LockStep => "lockStep",
                            }))),
                            ("scaleMode".to_owned(), NativeDatumValue::String(Self::native_bounded_string(match info.scale_mode {
                                crate::director::enums::FlashScaleMode::ShowAll => "showAll",
                                crate::director::enums::FlashScaleMode::NoScale => "noScale",
                                crate::director::enums::FlashScaleMode::AutoSize => "autoSize",
                                crate::director::enums::FlashScaleMode::ExactFit => "exactFit",
                                crate::director::enums::FlashScaleMode::NoBorder => "noBorder",
                            }))),
                            ("streamMode".to_owned(), NativeDatumValue::String(Self::native_bounded_string(match info.stream_mode {
                                crate::director::enums::FlashStreamMode::Frame => "frame",
                                crate::director::enums::FlashStreamMode::Idle => "idle",
                                crate::director::enums::FlashStreamMode::Manual => "manual",
                            }))),
                            ("quality".to_owned(), NativeDatumValue::String(Self::native_bounded_string(match info.quality {
                                crate::director::enums::FlashQuality::AutoHigh => "autoHigh",
                                crate::director::enums::FlashQuality::AutoMedium => "autoMedium",
                                crate::director::enums::FlashQuality::AutoLow => "autoLow",
                                crate::director::enums::FlashQuality::High => "high",
                                crate::director::enums::FlashQuality::Medium => "medium",
                                crate::director::enums::FlashQuality::Low => "low",
                            }))),
                            ("eventPassMode".to_owned(), NativeDatumValue::String(Self::native_bounded_string(match info.event_pass_mode {
                                crate::director::enums::FlashEventPassMode::PassAlways => "passAlways",
                                crate::director::enums::FlashEventPassMode::PassButton => "passButton",
                                crate::director::enums::FlashEventPassMode::PassNotButton => "passNotButton",
                                crate::director::enums::FlashEventPassMode::PassNever => "passNever",
                            }))),
                            ("clickMode".to_owned(), NativeDatumValue::String(Self::native_bounded_string(match info.click_mode {
                                crate::director::enums::FlashClickMode::BoundingBox => "boundingBox",
                                crate::director::enums::FlashClickMode::Opaque => "opaque",
                                crate::director::enums::FlashClickMode::Object => "object",
                            }))),
                            ("sourceFileName".to_owned(), NativeDatumValue::String(Self::native_bounded_string(&info.source_file_name))),
                            ("commonPlayer".to_owned(), NativeDatumValue::String(Self::native_bounded_string(&info.common_player))),
                            ("bgColor".to_owned(), NativeDatumValue::Int(info.bg_color as i32)),
                        ]);
                    }
                }
                CastMemberType::Palette(data) => {
                    fields.push(("colors".to_owned(), NativeDatumValue::Array(data.colors.iter().map(|color| {
                        NativeDatumValue::Array(vec![
                            NativeDatumValue::Int(color.0 as i32),
                            NativeDatumValue::Int(color.1 as i32),
                            NativeDatumValue::Int(color.2 as i32),
                        ])
                    }).collect())));
                    fields.push(("colorCount".to_owned(), NativeDatumValue::Int(data.colors.len() as i32)));
                }
                CastMemberType::Shockwave3d(data) => {
                    let info = &data.info;
                    fields.push(("regX".to_owned(), NativeDatumValue::Int(info.reg_point.0)));
                    fields.push(("regY".to_owned(), NativeDatumValue::Int(info.reg_point.1)));
                    fields.push(("dataSize".to_owned(), NativeDatumValue::Int(data.w3d_data.len() as i32)));
                    fields.push(("directToStage".to_owned(), NativeDatumValue::Bool(info.direct_to_stage)));
                    fields.push(("animationEnabled".to_owned(), NativeDatumValue::Bool(info.animation_enabled)));
                    fields.push(("preload".to_owned(), NativeDatumValue::Bool(info.preload)));
                    fields.push(("loop".to_owned(), NativeDatumValue::Bool(info.loops)));
                    fields.push(("duration".to_owned(), NativeDatumValue::Float(info.duration as f64)));
                    let rect = info.default_rect;
                    fields.extend([
                        ("width".to_owned(), NativeDatumValue::Int(rect.2 - rect.0)),
                        ("height".to_owned(), NativeDatumValue::Int(rect.3 - rect.1)),
                        ("rectLeft".to_owned(), NativeDatumValue::Int(rect.0)),
                        ("rectTop".to_owned(), NativeDatumValue::Int(rect.1)),
                        ("rectRight".to_owned(), NativeDatumValue::Int(rect.2)),
                        ("rectBottom".to_owned(), NativeDatumValue::Int(rect.3)),
                    ]);
                    if let Some(pos) = info.camera_position {
                        fields.push(("cameraPosition".to_owned(), NativeDatumValue::Array(vec![
                            NativeDatumValue::Float(pos.0 as f64), NativeDatumValue::Float(pos.1 as f64), NativeDatumValue::Float(pos.2 as f64),
                        ])));
                    }
                    if let Some(rot) = info.camera_rotation {
                        fields.push(("cameraRotation".to_owned(), NativeDatumValue::Array(vec![
                            NativeDatumValue::Float(rot.0 as f64), NativeDatumValue::Float(rot.1 as f64), NativeDatumValue::Float(rot.2 as f64),
                        ])));
                    }
                    if let Some(bg) = info.bg_color {
                        fields.push(("bgColor".to_owned(), NativeDatumValue::String(format!("rgb({},{},{})", bg.0, bg.1, bg.2))));
                    }
                    if let Some(ambient) = info.ambient_color {
                        fields.push(("ambientColor".to_owned(), NativeDatumValue::String(format!("rgb({},{},{})", ambient.0, ambient.1, ambient.2))));
                    }
                    fields.push(("hasScene".to_owned(), NativeDatumValue::Bool(data.parsed_scene.is_some())));
                }
                CastMemberType::Button(data) => {
                    fields.push(("hilite".to_owned(), NativeDatumValue::Bool(data.hilite)));
                }
                _ => {}
            }
        }
        Ok(NativeMemberSnapshot {
            member_ref: (member_ref.cast_lib, member_ref.cast_member),
            number: member.number,
            name: Self::native_bounded_string(&member.name),
            type_name: member.member_type.type_string().to_owned(),
            fields,
        })
    }

    fn native_script_snapshot(
        player: &DirPlayer,
        symbols: &SymbolTable,
        instance_ref: Option<ScriptInstanceRef>,
    ) -> Result<NativeScriptInstanceSnapshot, String> {
        let Some(instance_ref) = instance_ref else {
            return Ok(NativeScriptInstanceSnapshot {
                instance_id: None,
                script_ref: (0, 0),
                ancestor_id: None,
                properties: Vec::new(),
            });
        };
        let mut ancestors = HashSet::new();
        let mut cursor = Some(instance_ref.clone());
        let mut instance = None;
        for _ in 0..=20 {
            let Some(reference) = cursor.take() else { break };
            if !ancestors.insert(reference.id()) {
                return Err(format!("cyclic script ancestor {}", reference.id()));
            }
            let current = player
                .allocator
                .get_script_instance_opt(&reference)
                .ok_or_else(|| format!("foreign or stale script instance {}", reference.id()))?;
            if instance.is_none() {
                instance = Some(current);
            }
            cursor = current.ancestor.clone();
        }
        if cursor.is_some() {
            return Err("script ancestor depth limit exceeded".to_owned());
        }
        let instance = instance.expect("validated script instance");
        let mut budget = 4096;
        let mut properties = instance
            .properties
            .iter()
            .map(|(symbol, datum)| {
                let name = symbols
                    .display(symbol)
                    .map_err(|_| "foreign symbol in script property".to_owned())?
                    .to_owned();
                Ok((
                    name,
                    Self::native_datum_snapshot_with_budget(player, symbols, datum, &mut budget)?,
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        properties.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(NativeScriptInstanceSnapshot {
            instance_id: Some(instance.instance_id),
            script_ref: (instance.script.cast_lib, instance.script.cast_member),
            ancestor_id: instance.ancestor.as_ref().map(|ancestor| ancestor.id()),
            properties,
        })
    }

    pub(crate) fn dispatch_player_notifications(
        session: RuntimeSessionHandle,
        player_id: PlayerId,
    ) -> Result<(), JsValue> {
        let Some((drain_owner, batch)) = session
            .borrow_mut()
            .begin_player_notification_drain(player_id)
        else {
            return Ok(());
        };
        let mut push_error = None;
        let mut conversion_error: Option<(String, String)> = None;
        let mut retry_notifications = Vec::new();
        let mut notifications = batch.into_iter();
        while let Some(notification) = notifications.next() {
            if !notification.owner.same_identity(&drain_owner)
                || !session
                    .borrow()
                    .player_owner_matches(player_id, &notification.owner)
            {
                continue;
            }
            let prepared = session.borrow_mut().with_player(player_id, |context| -> Result<_, String> {
                let player = context.player;
                let symbols = context.symbols;
                match &notification.kind {
                    PlayerNotificationKind::ScoreChanged => Ok(Some(
                        NativePlayerNotificationKind::ScoreChanged(Self::native_score_snapshot(player)),
                    )),
                    PlayerNotificationKind::ChannelChanged(channel) => Ok(Some(
                        NativePlayerNotificationKind::ChannelChanged(
                            Self::native_channel_snapshot(player, *channel)?,
                        ),
                    )),
                    PlayerNotificationKind::ChannelNameChanged(channel) => {
                        let snapshot = Self::native_channel_snapshot(player, *channel)?;
                        Ok(Some(NativePlayerNotificationKind::ChannelNameChanged {
                            channel: *channel,
                            name: snapshot.display_name,
                        }))
                    }
                    PlayerNotificationKind::ChannelNamesChanged => {
                        let names = player
                            .movie
                            .score
                            .channels
                            .iter()
                            .enumerate()
                            .map(|(index, channel)| {
                                let name = if !channel.name.is_empty() {
                                    channel.name.clone()
                                } else if !channel.sprite.name.is_empty() {
                                    channel.sprite.name.clone()
                                } else {
                                    channel
                                        .sprite
                                        .member
                                        .as_ref()
                                        .and_then(|member_ref| {
                                            player.movie.cast_manager.find_member_by_ref(member_ref)
                                        })
                                        .map(|member| member.name.clone())
                                        .unwrap_or_default()
                                };
                                (index as i16, name)
                            })
                            .collect();
                        Ok(Some(NativePlayerNotificationKind::ChannelNamesChanged(names)))
                    }
                    PlayerNotificationKind::CastMemberChanged(member_ref) => Ok(Some(
                        NativePlayerNotificationKind::CastMemberChanged(
                            Self::native_member_snapshot(player, symbols, member_ref.clone())?,
                        ),
                    )),
                    PlayerNotificationKind::CastMemberListChanged(cast) => {
                        let cast_lib = player
                            .movie
                            .cast_manager
                            .get_cast(*cast)
                            .map_err(|error| error.message.clone())?;
                        let members = cast_lib
                            .members
                            .values()
                            .map(|member| {
                                Self::native_member_snapshot(
                                    player,
                                    symbols,
                                    CastMemberRef {
                                        cast_lib: *cast as i32,
                                        cast_member: member.number as i32,
                                    },
                                )
                            })
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok(Some(NativePlayerNotificationKind::CastMemberListChanged {
                            cast: *cast,
                            members,
                        }))
                    }
                    PlayerNotificationKind::CastMemberNameChanged(slot) => {
                        let names = player
                            .movie
                            .score
                            .channels
                            .iter()
                            .enumerate()
                            .filter_map(|(index, channel)| {
                                let member = channel.sprite.member.as_ref()?;
                                if member.cast_member as u32 != *slot {
                                    return None;
                                }
                                let name = if !channel.name.is_empty() {
                                    channel.name.clone()
                                } else if !channel.sprite.name.is_empty() {
                                    channel.sprite.name.clone()
                                } else {
                                    player
                                        .movie
                                        .cast_manager
                                        .find_member_by_ref(member)
                                        .map(|member| member.name.clone())
                                        .unwrap_or_default()
                                };
                                Some((index as i16, name))
                            })
                            .collect();
                        Ok(Some(NativePlayerNotificationKind::CastMemberNameChanged {
                            slot: *slot,
                            names,
                        }))
                    }
                    PlayerNotificationKind::DatumSnapshot(datum_ref) => Ok(Some(
                        NativePlayerNotificationKind::DatumSnapshot(Self::native_datum_snapshot(
                            player, symbols, datum_ref,
                        )?),
                    )),
                    PlayerNotificationKind::ScriptInstanceSnapshot(instance_ref) => Ok(Some(
                        NativePlayerNotificationKind::ScriptInstanceSnapshot(
                            Self::native_script_snapshot(player, symbols, instance_ref.clone())?,
                        ),
                    )),
                    PlayerNotificationKind::Host(event) => {
                        Ok(Some(NativePlayerNotificationKind::Host(event.clone())))
                    }
                    PlayerNotificationKind::HostBackpressure(capacity) => {
                        push_error = Some(crate::player::host_events::HostEventOverflow {
                            capacity: *capacity,
                        });
                        Ok(None)
                    }
                }
            });
            let Some(prepared) = prepared else {
                continue;
            };
            let prepared = match prepared {
                Ok(prepared) => prepared,
                Err(message) => {
                    conversion_error = Some((
                        Self::native_notification_name(&notification.kind).to_owned(),
                        message,
                    ));
                    break;
                }
            };
            let Some(kind) = prepared else {
                if push_error.is_some() {
                    break;
                }
                continue;
            };
            let native = NativePlayerNotification {
                player_id,
                owner: notification.owner.clone(),
                kind,
            };
            if let Err(error) = session.borrow_mut().push_native_player_notification(native) {
                push_error = Some(error);
                retry_notifications.push(notification);
                break;
            }
        }
        for notification in notifications {
            retry_notifications.push(notification);
        }
        if !retry_notifications.is_empty() {
            let pending = std::mem::take(&mut retry_notifications);
            let _ = session.borrow_mut().with_player(player_id, |context| {
                if context.player.owner.same_identity(&drain_owner) {
                    context.player.prepend_player_notifications(pending);
                }
            });
        }
        if let Some((notification, message)) = conversion_error {
            let error = crate::player::host_events::NativeNotificationError {
                player_id,
                owner: drain_owner.clone(),
                notification,
                message,
            };
            log::error!(
                "native notification conversion failed for player {}: {}",
                player_id,
                error.message
            );
            let mut runtime = session.borrow_mut();
            runtime.record_native_notification_error(error);
            runtime.finish_player_notification_drain(player_id, &drain_owner);
            return Err(JsValue::NULL);
        }
        if let Some(error) = push_error {
            let mut runtime = session.borrow_mut();
            runtime.with_player(player_id, |context| {
                if context.player.owner.same_identity(&drain_owner) {
                    context.player.host_event_backpressure = Some(error);
                }
            });
            runtime.cancel_host_backpressured_owner(player_id, &drain_owner);
            runtime.finish_player_notification_drain(player_id, &drain_owner);
            return Err(Self::native_host_event_overflow(error.capacity));
        }
        session
            .borrow_mut()
            .finish_player_notification_drain(player_id, &drain_owner);
        Ok(())
    }
    pub fn dispatch_datum_snapshot(_: &DatumRef, _: &SymbolTable, _: &DirPlayer) {}
    pub fn dispatch_script_instance_snapshot(_: Option<ScriptInstanceRef>, _: &SymbolTable, _: &DirPlayer) {}
    pub fn dispatch_schedule_timeout(_: &str, _: u32) {}
    pub fn dispatch_clear_timeout(_: &str) {}
    #[allow(dead_code)]
    pub fn dispatch_clear_timeouts() {}
    pub fn dispatch_movie_loaded(_: &DirectorFile) {}
    pub fn dispatch_movie_load_failed(_: &str, _: &str) {}
    pub fn dispatch_flash_member_loaded(_: i32, _: i32, _: i32, _: &[u8], _: u32, _: u32, _: bool, _: i32, _: &str) {}
    pub fn dispatch_flash_member_unloaded(_: i32, _: &str) {}
    pub fn dispatch_flash_reset_all(_: &str) {}
    pub fn dispatch_stage_size_changed(_: u32, _: u32, _: bool) {}
    pub fn dispatch_cast_name_changed(_: u32) {}
    pub fn dispatch_cast_list_changed() {}
    pub fn dispatch_cast_member_list_changed(_: u32) {}
    pub fn dispatch_cast_member_changed(_: CastMemberRef, _: &SymbolTable, _: &DirPlayer) {}
    pub fn on_cast_member_name_changed(_: u32) {}
    pub fn on_sprite_member_changed(_: i16) {}
    pub fn dispatch_score_changed() {}
    pub fn score_snapshot_for_player(_: &DirPlayer) -> Option<(js_sys::Object, String)> { None }
    pub fn dispatch_score_snapshot(_: js_sys::Object, _: &str) -> Result<(), JsValue> { Ok(()) }
    pub fn dispatch_channel_changed(_: i16) {}
    pub fn channel_snapshot_for_player(_: &DirPlayer, _: i16) -> (js_sys::Object, String) { (js_sys::Object::new(), String::new()) }
    pub fn dispatch_channel_snapshot(_: i16, _: js_sys::Object, _: &str) -> Result<(), JsValue> { Ok(()) }
    pub fn dispatch_frame_changed(_: u32) {}
    pub fn dispatch_debug_message(_: &str) -> Result<(), JsValue> { Ok(()) }
    pub fn dispatch_debug_message_owned(_: &str, _: &str) -> Result<(), JsValue> { Ok(()) }
    pub fn dispatch_debug_content(_: js_sys::Object) {}
    pub fn dispatch_debug_bitmap(_: u32, _: u32, _: &[u8]) {}
    pub fn dispatch_debug_datum(_: &DatumRef, _: &SymbolTable, _: &DirPlayer) {}
    pub fn dispatch_channel_name_changed(_: i16) {}
    pub fn channel_name_snapshots_for_member_slot(_: &DirPlayer, _: u32) -> Vec<(i16, String)> { vec![] }
    pub fn channel_name_snapshot_for_player(_: &DirPlayer, _: i16) -> Option<(String, String)> { None }
    pub fn dispatch_all_channel_names(_: &DirPlayer) {}
    pub fn channel_names_snapshot_for_player(_: &DirPlayer) -> (js_sys::Object, String) { (js_sys::Object::new(), String::new()) }
    pub fn dispatch_channel_names_snapshot(_: js_sys::Object, _: &str) -> Result<(), JsValue> { Ok(()) }
    pub fn dispatch_channel_name_snapshot(_: i16, _: &str, _: &str) -> Result<(), JsValue> { Ok(()) }
    pub fn dispatch_scope_list(_: &mut DirPlayer) {}
    pub fn dispatch_global_list(_: &SymbolTable, _: &DirPlayer) {}
    pub fn dispatch_debug_update(_: &SymbolTable, _: &mut DirPlayer) {}
    pub fn script_error_data(_: &DirPlayer, _: &ScriptError) -> js_sys::Object {
        js_sys::Object::new()
    }
    pub fn dispatch_script_error(_: &DirPlayer, _: &ScriptError) {}
    pub fn dispatch_script_error_data(_: js_sys::Object) -> Result<(), JsValue> { Ok(()) }
    pub fn dispatch_script_error_data_owned(_: &str, _: js_sys::Object) -> Result<(), JsValue> { Ok(()) }
    pub fn dispatch_breakpoint_list_changed() {}
    pub fn get_breakpoint_list(_: &DirPlayer) -> Vec<js_sys::Object> { vec![] }
    pub fn dispatch_script_error_cleared() {}
    pub fn dispatch_external_event(_: &str) {}
    pub fn get_cast_chunk_list_for(_: &DirPlayer, _: u32) -> js_sys::Object { unimplemented!() }
    pub fn get_movie_top_level_chunks(_: &DirPlayer) -> js_sys::Object { unimplemented!() }
    pub fn get_chunk_bytes(_: &DirPlayer, _: u32, _: u32) -> Option<Vec<u8>> { unimplemented!() }
    pub fn get_parsed_chunk(_: &DirPlayer, _: &SymbolTable, _: u32, _: u32) -> js_sys::Object { unimplemented!() }
    pub fn get_mini_member_snapshot(_: &CastMember) -> js_sys::Map { unimplemented!() }
    pub fn get_member_snapshot(_: &CastMember, _: u32, _: Option<&ScriptContext>, _: &SymbolTable, _: &DirPlayer) -> js_sys::Map { unimplemented!() }
    pub fn get_score_snapshot(_: &DirPlayer, _: &Score) -> js_sys::Map { unimplemented!() }
    pub fn get_channel_snapshot(_: &DirPlayer, _: &i16) -> js_sys::Map { unimplemented!() }
    fn get_channel_display_name(_: &i16, _: &DirPlayer) -> Option<String> { unimplemented!() }
    pub fn get_script_snapshot(_: &ScriptMember, _: &ScriptChunk, _: &ScriptContext, _: bool, _: u16, _: &SymbolTable) -> js_sys::Map { unimplemented!() }
    fn collect_cast_descendants(_: u32, _: &HashMap<u32, Vec<u32>>) -> std::collections::HashSet<u32> { unimplemented!() }
    fn build_children_map(_: &DirectorFile) -> HashMap<u32, Vec<u32>> { unimplemented!() }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod native_snapshot_tests {
    use super::*;
    use async_std::channel;
    use std::collections::VecDeque;
    use crate::director::lingo::datum::{Datum, DatumType};
    use crate::director::chunks::score::{ScoreChunk, ScoreChunkHeader};
    use crate::director::chunks::handler::{Bytecode, HandlerDef};
    use crate::director::chunks::script::ScriptChunk;
    use crate::director::enums::{FilmLoopInfo, ScriptType, ShapeInfo, ShapeType};
    use crate::director::lingo::{opcode::OpCode, script::ScriptContext};
    use crate::player::allocator::ScriptInstanceAllocatorTrait;
    use crate::player::bitmap::bitmap::{Bitmap, PaletteRef};
    use crate::player::cast_lib::CastLib;
    use crate::player::cast_member::{BitmapMember, CastMemberType, FilmLoopMember, FlashMember, PaletteMember, ScriptMember, ShapeMember, TextMember};
    use crate::player::geometry::IntRect;
    use crate::player::host_events::{NativeNotificationError, MAX_HOST_EVENTS};
    use crate::player::score::Score;
    use crate::player::script::Script;
    use crate::player::script::ScriptInstance;
    use crate::player::session::RuntimeSession;
    use crate::player::symbols::symbol_table::SymbolOwner;

    #[test]
    fn native_debug_description_is_utf8_safe_and_bounded() {
        let value = NativeDatumValue::String("é".repeat(100_000));
        let rendered = JsApi::native_value_debug(&value);
        assert!(rendered.len() <= 64 * 1024);
        assert!(rendered.ends_with("<truncated>"));
        assert!(std::str::from_utf8(rendered.as_bytes()).is_ok());
    }

    #[test]
    fn native_wide_debug_value_is_bounded() {
        let value = NativeDatumValue::Array(
            (0..20_000).map(NativeDatumValue::Int).collect(),
        );
        let rendered = JsApi::native_value_debug(&value);
        assert!(rendered.len() <= 64 * 1024);
        assert!(rendered.ends_with("<truncated>"));
    }

    #[test]
    fn native_nested_foreign_list_and_proplist_are_rejected() {
        let mut session = RuntimeSession::new(SymbolOwner { session: 910, generation: 1 });
        assert!(session.add_player(1, channel::unbounded().0));
        assert!(session.add_player(2, channel::unbounded().0));
        let foreign = session.with_player(2, |context| {
            context.player.alloc_datum(Datum::Int(7))
        }).unwrap();
        let (list, properties) = session.with_player(1, |context| {
            let list = context.player.alloc_datum(Datum::List(
                DatumType::List,
                VecDeque::from(vec![foreign.clone()]),
                false,
            ));
            let key = context.player.alloc_datum(Datum::String("foreign".to_owned()));
            let properties = context.player.alloc_datum(Datum::PropList(
                VecDeque::from(vec![(key, list.clone())]),
                false,
            ));
            (list, properties)
        }).unwrap();
        session.with_player(1, |context| {
            assert!(JsApi::native_datum_snapshot(context.player, context.symbols, &list).is_err());
            assert!(JsApi::native_datum_snapshot(context.player, context.symbols, &properties).is_err());
        }).unwrap();
    }

    #[test]
    fn native_foreign_symbol_and_stale_script_are_rejected() {
        let mut session = RuntimeSession::new(SymbolOwner { session: 911, generation: 1 });
        assert!(session.add_player(1, channel::unbounded().0));
        assert!(session.add_player(2, channel::unbounded().0));
        let foreign_symbol = crate::player::symbols::symbol_table::SymbolTable::new()
            .intern("foreign-symbol");
        let (symbol_datum, stale_instance) = session.with_player(1, |context| {
            let symbol_datum = context.player.alloc_datum(Datum::Symbol(foreign_symbol));
            let instance = context.player.allocator.alloc_script_instance(ScriptInstance {
                instance_id: 999,
                script: CastMemberRef { cast_lib: 1, cast_member: 1 },
                ancestor: None,
                properties: Default::default(),
                begin_sprite_called: false,
            });
            let stale_script = context.player.alloc_datum(Datum::ScriptInstanceRef(instance.clone()));
            (symbol_datum, (instance, stale_script))
        }).unwrap();
        session.with_player(1, |context| {
            assert!(JsApi::native_datum_snapshot(context.player, context.symbols, &symbol_datum).is_err());
        }).unwrap();
        let live_with_stale_ancestor = session.with_player(2, |context| {
            context.player.allocator.alloc_script_instance(ScriptInstance {
                instance_id: 1000,
                script: CastMemberRef { cast_lib: 1, cast_member: 2 },
                ancestor: Some(stale_instance.0.clone()),
                properties: Default::default(),
                begin_sprite_called: false,
            })
        }).unwrap();
        session.remove_player(1);
        assert!(session.with_player(2, |context| {
            JsApi::native_script_snapshot(context.player, context.symbols, Some(live_with_stale_ancestor))
        }).unwrap().is_err());
    }

    fn contains_native_marker(value: &NativeDatumValue, marker: &str) -> bool {
        match value {
            NativeDatumValue::Opaque(text) => text == marker,
            NativeDatumValue::Array(values) => values.iter().any(|value| contains_native_marker(value, marker)),
            NativeDatumValue::PropList(values, _) => values.iter().any(|(_, value)| contains_native_marker(value, marker)),
            _ => false,
        }
    }

    #[test]
    fn native_datum_snapshot_handles_cycles_depth_and_wide_lists() {
        let mut session = RuntimeSession::new(SymbolOwner { session: 912, generation: 1 });
        assert!(session.add_player(1, channel::unbounded().0));
        session.with_player(1, |context| {
            let cycle = context.player.alloc_datum(Datum::List(DatumType::List, VecDeque::new(), false));
            let cycle_ref = cycle.clone();
            if let Datum::List(_, values, _) = context.player.get_datum_mut(&cycle) {
                values.push_back(cycle_ref);
            }
            let cycle_snapshot = JsApi::native_datum_snapshot(context.player, context.symbols, &cycle).unwrap();
            assert!(contains_native_marker(&cycle_snapshot.value, "<cycle>"));

            let mut deep = DatumRef::Void;
            for _ in 0..24 {
                deep = context.player.alloc_datum(Datum::List(
                    DatumType::List,
                    VecDeque::from(vec![deep]),
                    false,
                ));
            }
            let deep_snapshot = JsApi::native_datum_snapshot(context.player, context.symbols, &deep).unwrap();
            assert!(contains_native_marker(&deep_snapshot.value, "<depth-limit>"));

            let wide_values: Vec<DatumRef> = (0..10_000)
                .map(|value| context.player.alloc_datum(Datum::Int(value)))
                .collect();
            let wide = context.player.alloc_datum(Datum::List(
                DatumType::List,
                VecDeque::from(wide_values),
                false,
            ));
            let wide_snapshot = JsApi::native_datum_snapshot(context.player, context.symbols, &wide).unwrap();
            let NativeDatumValue::Array(values) = wide_snapshot.value else { panic!("wide list must remain an array") };
            assert!(values.len() <= 4097);
            assert!(values.iter().any(|value| matches!(value, NativeDatumValue::Opaque(text) if text == "<omitted-tail>")));
        }).unwrap();
    }

    #[test]
    fn native_member_snapshot_uses_filmloop_score_and_typed_media_fields() {
        let mut session = RuntimeSession::new(SymbolOwner { session: 913, generation: 1 });
        assert!(session.add_player(1, channel::unbounded().0));
        session.with_player(1, |context| {
            context.player.movie.score.frame_count = Some(2);
            let mut filmloop_score = Score::empty();
            filmloop_score.frame_count = Some(37);
            let filmloop = CastMember::new(1, CastMemberType::FilmLoop(FilmLoopMember {
                info: FilmLoopInfo {
                    reg_point: (0, 0), width: 1, height: 1, center: 0,
                    crop: 0, sound: 0, loops: 0,
                },
                score_chunk: ScoreChunk {
                    header: ScoreChunkHeader {
                        total_length: 0, unk1: 0, unk2: 0, entry_count: 0,
                        unk3: 0, entry_size_sum: 0,
                    },
                    entries: Vec::new(), frame_intervals: Vec::new(),
                    frame_data: Default::default(), sprite_details: Default::default(),
                },
                score: filmloop_score,
                current_frame: 1,
                initial_rect: IntRect { left: 0, top: 0, right: 1, bottom: 1 },
                cached_total_frames: Some(37),
            }));
            let mut text = TextMember::new();
            text.text = "typed text".to_owned();
            let text = CastMember::new(2, CastMemberType::Text(text));
            let bitmap_ref = context.player.bitmap_manager.add_bitmap(Bitmap::new(
                3, 4, 32, 32, 8, PaletteRef::Default,
            ));
            let bitmap = CastMember::new(3, CastMemberType::Bitmap(BitmapMember {
                image_ref: bitmap_ref, ..BitmapMember::default()
            }));
            let flash = CastMember::new(4, CastMemberType::Flash(FlashMember {
                data: vec![1, 2, 3], reg_point: (2, 3), flash_info: None,
            }));
            let palette = CastMember::new(5, CastMemberType::Palette(PaletteMember::new()));
            let mut cast = CastLib::test_external(1, 0);
            cast.members.insert(1, filmloop);
            cast.members.insert(2, text);
            cast.members.insert(3, bitmap);
            cast.members.insert(4, flash);
            cast.members.insert(5, palette);
            context.player.movie.cast_manager.casts.push(cast);

            let filmloop_snapshot = JsApi::native_member_snapshot(
                context.player,
                context.symbols,
                CastMemberRef { cast_lib: 1, cast_member: 1 },
            ).unwrap();
            let score = filmloop_snapshot.fields.iter().find(|(key, _)| key == "score").unwrap();
            let NativeDatumValue::PropList(score_fields, _) = &score.1 else { panic!("filmloop score must be owned data") };
            assert!(score_fields.iter().any(|(key, value)| key == "frameCount" && matches!(value, NativeDatumValue::Int(37))));
            assert!(!score_fields.iter().any(|(key, value)| key == "frameCount" && matches!(value, NativeDatumValue::Int(2))));

            let typed = [
                (2, "text"), (3, "paletteRef"), (4, "dataSize"), (5, "colors"),
            ];
            for (member, field) in typed {
                let snapshot = JsApi::native_member_snapshot(
                    context.player,
                    context.symbols,
                    CastMemberRef { cast_lib: 1, cast_member: member },
                ).unwrap();
                assert!(snapshot.fields.iter().any(|(key, _)| key == field), "missing {field} for member {member}");
            }
            let text_snapshot = JsApi::native_member_snapshot(
                context.player,
                context.symbols,
                CastMemberRef { cast_lib: 1, cast_member: 2 },
            ).unwrap();
            assert!(text_snapshot.fields.iter().any(|(key, value)| {
                key == "htmlStyledSpans" && matches!(value, NativeDatumValue::Array(_))
            }));
        }).unwrap();
    }

    #[test]
    fn native_member_snapshot_includes_script_shape_and_checked_errors() {
        let mut session = RuntimeSession::new(SymbolOwner { session: 915, generation: 1 });
        assert!(session.add_player(1, channel::unbounded().0));
        session.with_player(1, |context| {
            let handler = HandlerDef {
                name_id: 0,
                bytecode_array: vec![
                    Bytecode::new(OpCode::PushInt8, 7, 0),
                    Bytecode::new(OpCode::SetGlobal, 0, 1),
                    Bytecode::new(OpCode::Ret, 0, 2),
                ],
                bytecode_index_map: fxhash::FxHashMap::default(),
                argument_name_ids: vec![1],
                local_name_ids: Vec::new(),
                global_name_ids: Vec::new(),
                compiled_ir: std::cell::RefCell::new(None),
            };
            let script_chunk = ScriptChunk {
                script_number: 7,
                literals: Vec::new(),
                handlers: vec![handler.clone()],
                property_name_ids: Vec::new(),
                property_defaults: HashMap::new(),
            };
            let handler_symbol = context.symbols.intern("onTest");
            let mut script_handlers = fxhash::FxHashMap::default();
            script_handlers.insert(handler_symbol.clone(), std::rc::Rc::new(handler));
            let script = std::rc::Rc::new(Script {
                member_ref: CastMemberRef { cast_lib: 1, cast_member: 1 },
                name: "native-script".to_owned(),
                chunk: script_chunk,
                script_type: ScriptType::Movie,
                handlers: script_handlers,
                handler_names_raw: vec!["onTest".to_owned()],
                handler_names: vec![handler_symbol],
                properties: std::cell::RefCell::new(fxhash::FxHashMap::default()),
            });
            let mut cast = CastLib::test_external(1, 0);
            cast.lctx = Some(ScriptContext {
                names: vec!["onTest".to_owned(), "arg".to_owned()],
                scripts: HashMap::new(),
            });
            cast.scripts.insert(1, script);
            cast.members.insert(
                1,
                CastMember::new(
                    1,
                    CastMemberType::Script(ScriptMember {
                        script_id: 7,
                        script_type: ScriptType::Movie,
                        name: "native-script".to_owned(),
                    }),
                ),
            );
            cast.members.insert(
                2,
                CastMember::new(
                    2,
                    CastMemberType::Shape(ShapeMember {
                        shape_info: ShapeInfo {
                            shape_type: ShapeType::OvalRect,
                            rect_top: 3,
                            rect_left: 4,
                            rect_bottom: 23,
                            rect_right: 44,
                            pattern: 9,
                            fore_color: 12,
                            back_color: 13,
                            fill_type: 1,
                            line_thickness: 3,
                            line_direction: 2,
                        },
                        script_id: 0,
                        member_script_ref: None,
                    }),
                ),
            );
            context.player.movie.cast_manager.casts.push(cast);

            let script_snapshot = JsApi::native_member_snapshot(
                context.player,
                context.symbols,
                CastMemberRef { cast_lib: 1, cast_member: 1 },
            ).unwrap();
            let script_value = script_snapshot
                .fields
                .iter()
                .find(|(key, _)| key == "script")
                .map(|(_, value)| value)
                .expect("script field");
            let script_fields = match script_value {
                NativeDatumValue::PropList(fields, _) => fields,
                _ => panic!("script field must be a prop list"),
            };
            assert!(script_fields.iter().any(|(key, value)| {
                key == "script_type" && matches!(value, NativeDatumValue::String(kind) if kind == "movie")
            }));
            assert!(script_fields.iter().any(|(key, value)| {
                key == "script_syntax" && matches!(value, NativeDatumValue::String(kind) if kind == "lingo")
            }));
            let handlers_value = script_fields
                .iter()
                .find(|(key, _)| key == "handlers")
                .map(|(_, value)| value)
                .expect("handlers field");
            let handlers = match handlers_value {
                NativeDatumValue::Array(handlers) => handlers,
                _ => panic!("handlers must be an array"),
            };
            let handler_fields = match &handlers[0] {
                NativeDatumValue::PropList(fields, _) => fields,
                _ => panic!("handler must be a prop list"),
            };
            assert!(handler_fields.iter().any(|(key, value)| {
                key == "name" && matches!(value, NativeDatumValue::String(name) if name == "onTest")
            }));
            assert!(handler_fields.iter().any(|(key, value)| {
                key == "args" && matches!(value, NativeDatumValue::Array(args) if args.iter().any(|arg| matches!(arg, NativeDatumValue::String(name) if name == "arg")))
            }));
            assert!(handler_fields.iter().any(|(key, value)| {
                key == "bytecode" && matches!(value, NativeDatumValue::Array(bytecode) if !bytecode.is_empty())
            }));
            assert!(handler_fields.iter().any(|(key, value)| {
                key == "lingo"
                    && matches!(value, NativeDatumValue::Array(lingo) if lingo.iter().any(|line| matches!(line,
                        NativeDatumValue::PropList(fields, _) if fields.iter().any(|(field, value)| {
                            field == "text"
                                && matches!(value, NativeDatumValue::String(text) if text.contains("onTest") && text.contains('7'))
                        })
                    )))
            }));
            assert!(handler_fields.iter().any(|(key, value)| {
                key == "bytecodeToLine"
                    && matches!(value, NativeDatumValue::PropList(entries, _) if !entries.is_empty())
            }));

            let shape_snapshot = JsApi::native_member_snapshot(
                context.player,
                context.symbols,
                CastMemberRef { cast_lib: 1, cast_member: 2 },
            ).unwrap();
            let get_shape_int = |key: &str| {
                shape_snapshot.fields.iter().find_map(|(field, value)| {
                    (field == key).then_some(value)
                })
            };
            assert!(matches!(get_shape_int("shapeType"), Some(NativeDatumValue::String(value)) if value == "roundRect"));
            assert!(matches!(get_shape_int("width"), Some(NativeDatumValue::Int(40))));
            assert!(matches!(get_shape_int("height"), Some(NativeDatumValue::Int(20))));
            assert!(matches!(get_shape_int("lineSize"), Some(NativeDatumValue::Int(2))));
            assert!(matches!(get_shape_int("filled"), Some(NativeDatumValue::Bool(true))));
            assert!(matches!(get_shape_int("pattern"), Some(NativeDatumValue::Int(9))));

            let mut missing_cast = CastLib::test_external(2, 0);
            missing_cast.lctx = Some(ScriptContext { names: vec![], scripts: HashMap::new() });
            missing_cast.members.insert(1, CastMember::new(1, CastMemberType::Script(ScriptMember {
                script_id: 7, script_type: ScriptType::Movie, name: "missing".to_owned(),
            })));
            context.player.movie.cast_manager.casts.push(missing_cast);
            let missing = JsApi::native_member_snapshot(
                context.player,
                context.symbols,
                CastMemberRef { cast_lib: 2, cast_member: 1 },
            ).expect_err("missing registered script must be reported");
            assert!(missing.contains("missing registered script"));

            let malformed_handler = HandlerDef {
                name_id: 0,
                bytecode_array: vec![Bytecode::new(OpCode::GetGlobal, 99, 0)],
                bytecode_index_map: fxhash::FxHashMap::default(),
                argument_name_ids: Vec::new(),
                local_name_ids: Vec::new(),
                global_name_ids: Vec::new(),
                compiled_ir: std::cell::RefCell::new(None),
            };
            let malformed_script = std::rc::Rc::new(Script {
                member_ref: CastMemberRef { cast_lib: 3, cast_member: 1 },
                name: "malformed".to_owned(),
                chunk: ScriptChunk {
                    script_number: 8,
                    literals: Vec::new(),
                    handlers: vec![malformed_handler],
                    property_name_ids: Vec::new(),
                    property_defaults: HashMap::new(),
                },
                script_type: ScriptType::Movie,
                handlers: fxhash::FxHashMap::default(),
                handler_names_raw: Vec::new(),
                handler_names: Vec::new(),
                properties: std::cell::RefCell::new(fxhash::FxHashMap::default()),
            });
            let mut malformed_cast = CastLib::test_external(3, 0);
            malformed_cast.lctx = Some(ScriptContext {
                names: vec!["badHandler".to_owned()],
                scripts: HashMap::new(),
            });
            malformed_cast.scripts.insert(1, malformed_script);
            malformed_cast.members.insert(1, CastMember::new(1, CastMemberType::Script(ScriptMember {
                script_id: 8,
                script_type: ScriptType::Movie,
                name: "malformed".to_owned(),
            })));
            context.player.movie.cast_manager.casts.push(malformed_cast);
            let malformed = JsApi::native_member_snapshot(
                context.player,
                context.symbols,
                CastMemberRef { cast_lib: 3, cast_member: 1 },
            ).expect_err("invalid script operand must be reported");
            assert!(malformed.contains("invalid name operand"));
        }).unwrap();
    }

    #[test]
    fn native_error_and_mailbox_state_is_bounded_across_reset_remove() {
        let mut session = RuntimeSession::new(SymbolOwner { session: 914, generation: 1 });
        assert!(session.add_player(1, channel::unbounded().0));
        assert!(session.add_player(2, channel::unbounded().0));
        let owner = session.with_player(1, |context| context.player.owner.clone()).unwrap();
        session.record_native_notification_error(NativeNotificationError {
            player_id: 1,
            owner: owner.clone(),
            notification: "DatumSnapshot".to_owned(),
            message: "foreign".to_owned(),
        });
        session.push_native_player_notification(NativePlayerNotification {
            player_id: 1,
            owner: owner.clone(),
            kind: NativePlayerNotificationKind::Host(HostEvent::FrameChanged { frame: 1 }),
        }).unwrap();
        let replacement = session.reset_player_owned(1, &owner).unwrap();
        assert!(session.take_native_notification_error(1).is_none());
        assert!(session.take_native_player_notifications(1).is_empty());

        // Retired-owner writes are rejected and do not replace the one-slot error.
        session.record_native_notification_error(NativeNotificationError {
            player_id: 1,
            owner,
            notification: "retired".to_owned(),
            message: "stale".to_owned(),
        });
        assert!(session.take_native_notification_error(1).is_none());
        session.record_native_notification_error(NativeNotificationError {
            player_id: 1,
            owner: replacement.clone(),
            notification: "current".to_owned(),
            message: "bounded".to_owned(),
        });
        assert_eq!(session.take_native_notification_error(1).unwrap().notification, "current");

        // Player 1 saturation cannot consume player 2's independent quota.
        for _ in 0..MAX_HOST_EVENTS {
            session.push_native_player_notification(NativePlayerNotification {
                player_id: 1,
                owner: replacement.clone(),
                kind: NativePlayerNotificationKind::Host(HostEvent::FrameChanged { frame: 2 }),
            }).unwrap();
        }
        let owner2 = session.with_player(2, |context| context.player.owner.clone()).unwrap();
        assert!(session.push_native_player_notification(NativePlayerNotification {
            player_id: 2,
            owner: owner2,
            kind: NativePlayerNotificationKind::Host(HostEvent::FrameChanged { frame: 3 }),
        }).is_ok());
        assert!(session.remove_player(1).is_some());
        assert!(session.take_native_player_notifications(1).is_empty());
        assert!(session.take_native_notification_error(1).is_none());
    }
}

pub trait JsSerializable {
    fn to_js_object(&self) -> js_sys::Object;
}

pub trait JsUtils {
    fn str_set(&self, key: &str, value: &JsValue);
}

impl JsSerializable for js_sys::Map {
    fn to_js_object(&self) -> js_sys::Object {
        return js_sys::Object::from_entries(self).unwrap();
    }
}

impl JsUtils for js_sys::Map {
    fn str_set(&self, key: &str, value: &JsValue) {
        self.set(&safe_js_string(key), value);
    }
}

fn datum_to_js_bridge(datum_ref: &DatumRef, symbols: &SymbolTable, player: &DirPlayer, depth: u8) -> JsBridgeDatum {
    let datum = player.get_datum(datum_ref);
    concrete_datum_to_js_bridge(datum, symbols, player, depth)
}

fn concrete_datum_to_js_bridge(datum: &Datum, symbols: &SymbolTable, player: &DirPlayer, depth: u8) -> JsBridgeDatum {
    if depth > 20 {
        let map = js_sys::Map::new();
        map.str_set("debugDescription", &safe_js_string("TOO DEEP"));
        return map.to_js_object();
    }
    let map = js_sys::Map::new();
    let formatted_value = format_concrete_datum(datum, symbols, player)
        .unwrap_or_else(|error| format!("<invalid datum: {}>", error.message));
    map.str_set(
        "debugDescription",
        &ascii_safe(&formatted_value).to_js_value(),
    );
    match datum {
        Datum::String(val) => {
            map.str_set("type", &safe_js_string("string"));
            map.str_set("value", &safe_js_string(&ascii_safe(val)));
        }
        Datum::Int(val) => {
            map.str_set("type", &safe_js_string("number"));
            map.str_set("value", &JsValue::from_f64(*val as f64));
        }
        Datum::Symbol(val) => {
            map.str_set("type", &safe_js_string("symbol"));
            map.str_set("value", &safe_js_string(symbols.display(val).unwrap_or("<foreign-symbol>")));
        }
        Datum::List(_, item_refs, _) => {
            map.str_set("type", &safe_js_string("list"));
            map.str_set(
                "items",
                &item_refs
                    .iter()
                    .map(|x| x.unwrap())
                    .collect_vec()
                    .to_js_value(),
            );
        }
        Datum::VarRef(_) => {
            map.str_set("type", &safe_js_string("var_ref"));
        }
        Datum::Float(val) => {
            map.str_set("type", &safe_js_string("number"));
            map.str_set("numericValue", &JsValue::from_f64(*val as f64));
            map.str_set("value", &safe_js_string(&format_float_with_precision(*val, player)));
        }
        Datum::Void => {
            map.str_set("type", &safe_js_string("void"));
        }
        Datum::CastLib(val) => {
            map.str_set("type", &safe_js_string("castLib"));
            map.str_set("value", &JsValue::from_f64(*val as f64));
        }
        Datum::Stage => {
            map.str_set("type", &safe_js_string("stage"));
        }
        Datum::PropList(properties, sorted) => {
            map.str_set("type", &safe_js_string("propList"));
            let props_map = js_sys::Map::new();
            for (k, v) in properties.iter() {
                let key_str = format_datum(k, symbols, player)
                    .unwrap_or_else(|error| format!("<invalid datum: {}>", error.message));
                props_map.set(&safe_js_string(&key_str), &v.unwrap().to_js_value());
            }
            map.str_set("properties", &props_map.to_js_object());
            map.str_set("sorted", &JsValue::from_bool(*sorted));
        }
        Datum::StringChunk(..) => {
            map.str_set("type", &safe_js_string("stringChunk"));
        }
        Datum::ScriptRef(_) => {
            map.str_set("type", &safe_js_string("scriptRef"));
        }
        Datum::ScriptInstanceRef(instance_id) => {
            map.str_set("type", &safe_js_string("scriptInstance"));
            let instance = player.allocator.get_script_instance(&instance_id);
            let ancestor_id = &instance.ancestor;
            match ancestor_id {
                Some(ancestor_id) => {
                    map.str_set("ancestor", &(**ancestor_id).to_js_value());
                }
                None => map.str_set("ancestor", &JsValue::NULL),
            }

            let props_map = js_sys::Map::new();
            for (k, v) in instance.properties.iter() {
                props_map.set(&safe_js_string(symbols.display(k).unwrap_or("<foreign-symbol>")), &v.unwrap().to_js_value());
            }
            map.str_set("properties", &props_map.to_js_object());
        }
        Datum::CastMember(_) => {
            map.str_set("type", &safe_js_string("castMember"));
        }
        Datum::SpriteRef(_) => {
            map.str_set("type", &safe_js_string("spriteRef"));
        }
        Datum::Rect(vals, flags) => {
            let x1 = Datum::inline_component_to_datum(vals[0], Datum::inline_is_float(*flags, 0));
            let y1 = Datum::inline_component_to_datum(vals[1], Datum::inline_is_float(*flags, 1));
            let x2 = Datum::inline_component_to_datum(vals[2], Datum::inline_is_float(*flags, 2));
            let y2 = Datum::inline_component_to_datum(vals[3], Datum::inline_is_float(*flags, 3));

            map.str_set("type", &safe_js_string("Rect"));
            map.str_set("left", &concrete_datum_to_js_bridge(&x1, symbols, player, depth + 1));
            map.str_set("top", &concrete_datum_to_js_bridge(&y1, symbols, player, depth + 1));
            map.str_set("right", &concrete_datum_to_js_bridge(&x2, symbols, player, depth + 1));
            map.str_set("bottom", &concrete_datum_to_js_bridge(&y2, symbols, player, depth + 1));
            map.str_set("value", &safe_js_string(&format!(
                "rect({}, {}, {}, {})",
                if Datum::inline_is_float(*flags, 0) { format!("{:.4}", vals[0]) } else { format!("{}", vals[0] as i32) },
                if Datum::inline_is_float(*flags, 1) { format!("{:.4}", vals[1]) } else { format!("{}", vals[1] as i32) },
                if Datum::inline_is_float(*flags, 2) { format!("{:.4}", vals[2]) } else { format!("{}", vals[2] as i32) },
                if Datum::inline_is_float(*flags, 3) { format!("{:.4}", vals[3]) } else { format!("{}", vals[3] as i32) },
            )));
        }
        Datum::Point(vals, flags) => {
            let x = Datum::inline_component_to_datum(vals[0], Datum::inline_is_float(*flags, 0));
            let y = Datum::inline_component_to_datum(vals[1], Datum::inline_is_float(*flags, 1));

            map.str_set("type", &safe_js_string("Point"));
            map.str_set("x", &concrete_datum_to_js_bridge(&x, symbols, player, depth + 1));
            map.str_set("y", &concrete_datum_to_js_bridge(&y, symbols, player, depth + 1));
            map.str_set("value", &safe_js_string(&format!(
                "point({}, {})",
                if Datum::inline_is_float(*flags, 0) { format!("{:.4}", vals[0]) } else { format!("{}", vals[0] as i32) },
                if Datum::inline_is_float(*flags, 1) { format!("{:.4}", vals[1]) } else { format!("{}", vals[1] as i32) },
            )));
        }
        Datum::CursorRef(cursor_ref) => {
            map.str_set("type", &safe_js_string("cursorRef"));
            match cursor_ref {
                CursorRef::System(id) => {
                    map.str_set("cursorType", &safe_js_string("system"));
                    map.str_set("id", &JsValue::from(*id));
                }
                CursorRef::Member(member_ref) => {
                    map.str_set("cursorType", &safe_js_string("member"));
                    map.str_set("memberRef", &member_ref.to_js_value());
                }
            }
        }
        Datum::TimeoutRef(name) => {
            map.str_set("type", &safe_js_string("timeout"));
            map.str_set("name", &safe_js_string(name));
        }
        Datum::TimeoutFactory => {
            map.str_set("type", &safe_js_string("timeoutFactory"));
        }
        Datum::TimeoutInstance(ti) => {
            map.str_set("type", &safe_js_string("timeoutInstance"));
            map.str_set("name", &safe_js_string(&ti.name));
        }
        Datum::ColorRef(color_ref) => {
            map.str_set("type", &safe_js_string("colorRef"));
            match color_ref {
                ColorRef::PaletteIndex(i) => {
                    map.str_set("paletteIndex", &JsValue::from(*i));
                }
                ColorRef::Rgb(r, g, b) => {
                    map.str_set("r", &JsValue::from(*r));
                    map.str_set("g", &JsValue::from(*g));
                    map.str_set("b", &JsValue::from(*b));
                }
            }
        }
        Datum::BitmapRef(bitmap_ref) => {
            map.str_set("type", &safe_js_string("bitmapRef"));
            if let Some(bitmap) = player.bitmap_manager.get_bitmap_handle(bitmap_ref) {
                map.str_set("width", &JsValue::from(bitmap.width));
                map.str_set("height", &JsValue::from(bitmap.height));
                map.str_set("bitDepth", &JsValue::from(bitmap.bit_depth));
            }
        }
        Datum::PaletteRef(palette_ref) => {
            map.str_set("type", &safe_js_string("paletteRef"));
            map.str_set("value", &palette_ref.to_js_value());
        }
        Datum::Xtra(name) => {
            map.str_set("type", &safe_js_string("xtra"));
            map.str_set("name", &safe_js_string(name));
        }
        Datum::XtraInstance(name, instance_id) => {
            map.str_set("type", &safe_js_string("xtraInstance"));
            map.str_set("name", &safe_js_string(name));
            map.str_set("instanceId", &JsValue::from(*instance_id));
        }
        Datum::Matte(..) => {
            map.str_set("type", &safe_js_string("matte"));
        }
        Datum::Null => {
            map.str_set("type", &safe_js_string("null"));
        }
        Datum::PlayerRef => {
            map.str_set("type", &safe_js_string("playerRef"));
        }
        Datum::MovieRef => {
            map.str_set("type", &safe_js_string("movieRef"));
        }
        Datum::MouseRef => {
            map.str_set("type", &safe_js_string("mouseRef"));
        }
        Datum::SoundRef(sound_id) => {
            map.str_set("type", &safe_js_string("sound"));
            map.str_set("id", &JsValue::from(*sound_id));
        }
        Datum::SoundChannel(channel_id) => {
            map.str_set("type", &safe_js_string("soundChannel"));
            map.str_set("channel", &JsValue::from(*channel_id));
        }
        Datum::XmlRef(id) => {
            map.str_set("type", &safe_js_string("xmlRef"));
            map.str_set("id", &JsValue::from_f64(*id as f64));
        }
        Datum::JsObjectRef(handle) => {
            map.str_set("type", &safe_js_string("jsObjectRef"));
            map.str_set("id", &JsValue::from_f64(handle.id() as f64));
        }
        Datum::DateRef(_) => {
            map.str_set("type", &safe_js_string("date"));
        }
        Datum::MathRef(_) => {
            map.str_set("type", &safe_js_string("math"));
        }
        Datum::Vector(vec) => {
            map.str_set("type", &safe_js_string("vector"));
            let vec_array = js_sys::Array::new();
            for val in vec.iter() {
                vec_array.push(&JsValue::from_f64(*val as f64));
            }
            map.str_set("values", &vec_array);
        }
        Datum::Media(_) => {
            map.str_set("type", &safe_js_string("media"));
        }
        Datum::JavaScript(data) => {
            map.str_set("type", &safe_js_string("javascript"));
            map.str_set("size", &JsValue::from(data.len() as f64));
            map.str_set("bytes", &js_sys::Uint8Array::from(&data[..]));
        }
        Datum::FlashObjectRef(flash_ref) => {
            map.str_set("type", &safe_js_string("flashObject"));
            map.str_set("value", &safe_js_string(&flash_ref.path));
        }
        Datum::Shockwave3dObjectRef(s3d_ref) => {
            map.str_set("type", &safe_js_string("shockwave3dObject"));
            map.str_set("value", &safe_js_string(&format!(
                "{}(\"{}\")",
                s3d_ref.object_type.as_str(),
                symbols.display(&s3d_ref.name).unwrap_or("<foreign-symbol>"),
            )));
        }
        Datum::Transform3d(_) => {
            map.str_set("type", &safe_js_string("transform"));
        }
        Datum::HavokObjectRef(hk_ref) => {
            map.str_set("type", &safe_js_string("havokObject"));
            map.str_set("value", &safe_js_string(&format!(
                "{}(\"{}\")",
                hk_ref.object_type.as_str(),
                symbols.display(&hk_ref.name).unwrap_or("<foreign-symbol>"),
            )));
        }
        Datum::PhysXObjectRef(px_ref) => {
            map.str_set("type", &safe_js_string("physxObject"));
            map.str_set("value", &safe_js_string(&format!(
                "{}(\"{}\")",
                px_ref.object_type.as_str(),
                symbols.display(&px_ref.name).unwrap_or("<foreign-symbol>"),
            )));
        }
        Datum::VectorVertexRef(member_ref, index) => {
            map.str_set("type", &safe_js_string("vectorVertexRef"));
            map.str_set("value", &safe_js_string(&format!(
                "vertex[{}] of member({}, {})", index + 1, member_ref.cast_member, member_ref.cast_lib
            )));
        }
    }
    return map.to_js_object();
}

/// Serialize a datum while the caller holds the owning session context.
/// Dynamic symbols are deliberately resolved through that context's table;
/// callers must not substitute a process-global or freshly-created table.
pub(crate) fn datum_to_js_bridge_with_symbols(
    datum_ref: &DatumRef,
    symbols: &SymbolTable,
    player: &DirPlayer,
) -> JsBridgeDatum {
    datum_to_js_bridge(datum_ref, symbols, player, 0)
}

pub trait ToJsValue {
    fn to_js_value(&self) -> JsValue;
}

impl ToJsValue for String {
    fn to_js_value(&self) -> JsValue {
        safe_js_string(self)
    }
}

impl ToJsValue for u8 {
    fn to_js_value(&self) -> JsValue {
        JsValue::from_f64(*self as f64)
    }
}

impl ToJsValue for u32 {
    fn to_js_value(&self) -> JsValue {
        JsValue::from_f64(*self as f64)
    }
}

impl ToJsValue for usize {
    fn to_js_value(&self) -> JsValue {
        JsValue::from_f64(*self as f64)
    }
}

impl ToJsValue for u16 {
    fn to_js_value(&self) -> JsValue {
        JsValue::from_f64(*self as f64)
    }
}

impl ToJsValue for i16 {
    fn to_js_value(&self) -> JsValue {
        JsValue::from_f64(*self as f64)
    }
}

impl ToJsValue for PaletteRef {
    fn to_js_value(&self) -> JsValue {
        match self {
            PaletteRef::BuiltIn(id) => safe_js_string(&id.symbol().to_string()),
            PaletteRef::Member(member_ref) => safe_js_string(
                format!(
                    "(member {} of castLib {})",
                    member_ref.cast_member, member_ref.cast_lib
                )
                .as_str(),
            ),
            PaletteRef::Default => safe_js_string("#default"),
        }
    }
}
