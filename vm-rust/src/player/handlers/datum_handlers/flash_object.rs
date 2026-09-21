use std::collections::VecDeque;

use crate::{
    director::lingo::datum::{Datum, FlashObjectRef},
    player::{
        owner_key_string, DatumRef, DirPlayer, ScriptError,
        handlers::datum_handlers::date::DateObject,
        ownership::OwnerToken,
        session::{PlayerId, RuntimeSessionHandle},
        symbols::{builtin::BuiltInSymbol, symbol::Symbol, symbol_table::SymbolTable}
    }
};
use wasm_bindgen::prelude::*;
use log::warn;

// JS bridge names use the `dirplayer_` prefix so this fork's globals don't
// collide with stock Ruffle if both are loaded on the same page (e.g. via a
// browser extension). Matching JS-side definitions live in
// src/services/flashPlayerManager.ts::initFlashBridge.
#[wasm_bindgen]
extern "C" {
    // Per-sprite Flash bridge — see datum_handlers/sprite.rs for the full
    // signature comment. FlashObjectRef-driven calls go through these
    // externs after resolving the host sprite from the FlashObjectRef.
    #[wasm_bindgen(js_name = "dirplayer_ruffleGetVariable", catch)]
    fn ruffle_get_variable_global(sprite_num: i32, path: &str) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(js_name = "dirplayer_ruffleSetVariable", catch)]
    fn ruffle_set_variable_global(sprite_num: i32, path: &str, value: &str) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(js_name = "dirplayer_ruffleCallFunction", catch)]
    fn ruffle_call_function_global(sprite_num: i32, path: &str, args_xml: &str) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(js_name = "dirplayer_ruffleGetVariableOwnedForBinding", catch)]
    fn ruffle_get_variable_owned_for_binding(owner_key: &str, sprite_num: i32, path: &str, return_as_object: bool) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(js_name = "dirplayer_ruffleGetVariableOwnedAtGeneration", catch)]
    fn ruffle_get_variable_owned_at_generation(owner_key: &str, sprite_num: i32, generation: f64, path: &str, return_as_object: bool) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(js_name = "dirplayer_ruffleGetSpriteVariableOwnedAtGeneration", catch)]
    fn ruffle_get_sprite_variable_owned_at_generation(owner_key: &str, sprite_num: i32, generation: f64, path: &str) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(js_name = "dirplayer_isFlashInstanceReadyOwned", catch)]
    fn is_flash_instance_ready_owned(owner_key: &str, sprite_num: i32, generation: f64) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(js_name = "dirplayer_ruffleSetVariableOwnedAtGeneration", catch)]
    fn ruffle_set_variable_owned_at_generation(owner_key: &str, sprite_num: i32, generation: f64, path: &str, value: &str) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(js_name = "dirplayer_ruffleCallFunctionOwnedAtGeneration", catch)]
    fn ruffle_call_function_owned_at_generation(owner_key: &str, sprite_num: i32, generation: f64, path: &str, args_xml: &str) -> Result<JsValue, JsValue>;

    /// Forward a Director sprite mouse event into a Flash sprite's Ruffle
    /// instance. Called from the mouseDown/mouseUp command handlers. The
    /// SWF's own AS1 button handlers don't otherwise fire because Ruffle's
    /// canvas is hidden offscreen and never receives real browser clicks.
    /// `event_type` is "down", "up", or "move"; coordinates are sprite-local
    /// (origin at the sprite's top-left in Director stage coords).
    #[wasm_bindgen(js_name = "dirplayer_ruffleDispatchMouse", catch)]
    pub fn ruffle_dispatch_mouse_event_global(
        sprite_num: i32,
        event_type: &str,
        local_x: i32,
        local_y: i32,
        // Sprite display size, so JS can rebase sprite-local coords into the SWF's
        // internal (native) coordinate space — the sprite often shows the SWF scaled
        // (e.g. a 626x468 SWF displayed in a 600x320 sprite).
        sprite_w: i32,
        sprite_h: i32,
    ) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(js_name = "dirplayer_ruffleDispatchMouseOwned", catch)]
    fn ruffle_dispatch_mouse_event_owned_bridge(
        owner_key: &str,
        sprite_num: i32,
        generation: f64,
        event_type: &str,
        local_x: i32,
        local_y: i32,
        sprite_w: i32,
        sprite_h: i32,
    ) -> Result<JsValue, JsValue>;
}

pub(crate) fn ruffle_dispatch_mouse_event_owned(
    session: &RuntimeSessionHandle,
    player_id: PlayerId,
    owner: &OwnerToken,
    sprite_num: i32,
    generation: u64,
    event_type: &str,
    local_x: i32,
    local_y: i32,
    sprite_w: i32,
    sprite_h: i32,
) -> Result<(), ScriptError> {
    #[cfg(target_arch = "wasm32")]
    {
        let owner_key = owner_key_string(owner);
        if generation == 0 || generation > 9_007_199_254_740_991 {
            return Err(ScriptError::new_code(
                crate::player::ScriptErrorCode::InvalidReference,
                "Flash mouse dispatch has an invalid instance generation".to_owned(),
            ));
        }
        if !owner_key.is_empty() {
            let response = ruffle_dispatch_mouse_event_owned_bridge(
                &owner_key,
                sprite_num,
                generation as f64,
                event_type,
                local_x,
                local_y,
                sprite_w,
                sprite_h,
            ).map_err(|error| ScriptError::new(format!("Flash mouse dispatch failed: {error:?}")))?;
            decode_mouse_dispatch_response(response, generation)
        } else {
            Err(ScriptError::new("Flash mouse dispatch has no owner".to_owned()))
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let host = session
            .borrow()
            .native_flash(player_id)
            .and_then(|binding| binding.upgrade())
            .ok_or_else(|| ScriptError::new("native Flash mouse host is unavailable".to_owned()))?;
        host.borrow_mut().dispatch_mouse(
            session,
            player_id,
            owner,
            sprite_num as i16,
            generation,
            event_type,
            local_x,
            local_y,
            sprite_w,
            sprite_h,
        )
    }
}

pub(crate) fn decode_mouse_dispatch_response(value: JsValue, expected_generation: u64) -> Result<(), ScriptError> {
    let ok = js_sys::Reflect::get(&value, &JsValue::from_str("ok"))
        .map_err(|error| ScriptError::new(format!("Flash mouse response lookup failed: {error:?}")))?
        .as_bool()
        .ok_or_else(|| ScriptError::new("Flash mouse response has non-boolean ok".to_owned()))?;
    if !ok {
        let code = js_sys::Reflect::get(&value, &JsValue::from_str("code"))
            .ok()
            .and_then(|value| value.as_string())
            .unwrap_or_else(|| "host-error".to_owned());
        let message = js_sys::Reflect::get(&value, &JsValue::from_str("message"))
            .ok()
            .and_then(|value| value.as_string())
            .unwrap_or_else(|| format!("Flash mouse dispatch failed: {code}"));
        if matches!(code.as_str(), "unknown-owner" | "disposed-owner" | "stale-generation" | "invalid-generation") {
            return Err(ScriptError::new_code(
                crate::player::ScriptErrorCode::InvalidReference,
                message,
            ));
        }
        return Err(ScriptError::new(message));
    }
    let generation = js_sys::Reflect::get(&value, &JsValue::from_str("generation"))
        .map_err(|error| ScriptError::new(format!("Flash mouse generation lookup failed: {error:?}")))?
        .as_f64()
        .filter(|value| value.is_finite() && *value >= 1.0 && *value <= 9_007_199_254_740_991.0 && value.fract() == 0.0)
        .map(|value| value as u64)
        .ok_or_else(|| ScriptError::new("Flash mouse response has invalid generation".to_owned()))?;
    if generation != expected_generation {
        return Err(ScriptError::new_code(
            crate::player::ScriptErrorCode::InvalidReference,
            "Flash mouse response generation is stale".to_owned(),
        ));
    }
    Ok(())
}

/// Owned Flash work crosses the browser boundary only after the VM borrow has
/// ended. `BindGet` is the sole operation without an expected generation; a
/// not-ready response must capture its generation before retrying by generation.
#[derive(Clone, Debug)]
pub(crate) enum FlashOperation {
    BindGet { return_mode: FlashReturnMode },
    Get { return_mode: FlashReturnMode },
    SpriteGet { return_mode: FlashReturnMode },
    Set { value: String },
    Call { args_xml: String },
}

#[derive(Clone, Debug)]
pub(crate) enum FlashReturnMode {
    Scalar,
    Object { fallback_path: Option<String> },
}

impl FlashReturnMode {
    fn is_object(&self) -> bool {
        matches!(self, Self::Object { .. })
    }

    fn fallback_path<'a>(&'a self, request_path: &'a str) -> Option<&'a str> {
        match self {
            Self::Scalar => None,
            Self::Object { fallback_path } => fallback_path.as_deref().or(Some(request_path)),
        }
    }
}

fn is_canonical_stored_flash_path(path: &str) -> bool {
    const PREFIX: &str = "_level0.__dirplayer_ref_";
    let Some(suffix) = path.strip_prefix(PREFIX) else {
        return false;
    };
    if suffix.is_empty() || suffix.len() > 10 || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    suffix.parse::<u64>().is_ok_and(|value| value <= u32::MAX as u64)
}

#[derive(Clone, Debug)]
pub(crate) struct FlashRequest {
    pub(crate) player_id: u32,
    pub(crate) owner: OwnerToken,
    pub(crate) sprite_num: i32,
    pub(crate) expected_generation: Option<u64>,
    pub(crate) path: String,
    pub(crate) operation: FlashOperation,
    pub(crate) cast_lib: i32,
    pub(crate) cast_member: i32,
}

#[derive(Clone, Debug)]
pub(crate) enum FlashDecodedValue {
    Void,
    String(String),
    Int(i32),
    Float(f64),
    Bool(bool),
    Date(f64),
    Object(String),
    List(Vec<FlashDecodedValue>),
}

#[derive(Clone, Debug)]
pub(crate) struct FlashOwnedResponse {
    pub(crate) generation: u64,
    pub(crate) value: FlashDecodedValue,
}

#[derive(Clone, Debug)]
pub(crate) enum FlashRequestError {
    NotReady { generation: u64 },
    Script(ScriptError),
}

impl From<ScriptError> for FlashRequestError {
    fn from(error: ScriptError) -> Self {
        Self::Script(error)
    }
}

/// Find the sprite number that currently displays the given Flash cast
/// member. With per-sprite Ruffle instances we need a sprite-num key for
/// the JS bridge; FlashObjectRef only carries cast_lib/cast_member, so we
/// scan the score for the first sprite that references this member.
/// Most movies have at most one sprite per Flash member; storyscramble's
/// 3-tile case shares a member but those tiles don't use FlashObjectRef
/// (they only set frame), so this lookup is fine for the existing surface.
/// Resolve the Director sprite that backs a Flash object handle. A handle
/// taken from `getVariable(sprite(N), path, 0)` carries its origin sprite (see
/// FlashObjectRef::instance_id) and dispatches straight to it; older handles
/// only know their cast member and fall back to scanning the score. Preferring
/// the bound sprite keeps `gDemoFlash.play()` working even when the member
/// wasn't resolvable at the time the handle was captured.
pub fn resolve_flash_sprite(flash_ref: &FlashObjectRef) -> Option<i32> {
    if let Some(sn) = flash_ref.bound_sprite() {
        return Some(sn);
    }
    find_sprite_for_flash_member(flash_ref.cast_lib, flash_ref.cast_member)
}

pub fn find_sprite_for_flash_member(cast_lib: i32, cast_member: i32) -> Option<i32> {
    crate::player::reserve_player_ref(|player| {
        for channel in &player.movie.score.channels {
            if let Some(member_ref) = &channel.sprite.member {
                if member_ref.cast_lib == cast_lib && member_ref.cast_member == cast_member {
                    return Some(channel.number as i32);
                }
            }
        }
        None
    })
}

pub(crate) fn validate_owned_binding(
    player: &DirPlayer,
    flash_ref: &FlashObjectRef,
) -> Result<(String, u64, i32), ScriptError> {
    let owner_key = owner_key_string(&player.owner);
    let Some(handle_owner) = flash_ref.owner_key.as_deref() else {
        return Err(ScriptError::new_code(
            crate::player::ScriptErrorCode::InvalidReference,
            "Flash object is not bound to an owner".to_owned(),
        ));
    };
    if handle_owner != owner_key {
        return Err(ScriptError::new_code(
            crate::player::ScriptErrorCode::InvalidReference,
            "Flash object belongs to another owner".to_owned(),
        ));
    }
    let generation = flash_ref.instance_generation.ok_or_else(|| {
        ScriptError::new_code(
            crate::player::ScriptErrorCode::InvalidReference,
            "Flash object has no instance generation".to_owned(),
        )
    })?;
    if generation == 0 || generation > 9_007_199_254_740_991 {
        return Err(ScriptError::new_code(
            crate::player::ScriptErrorCode::InvalidReference,
            "Flash object has an invalid or unsafe instance generation".to_owned(),
        ));
    }
    let sprite_num = flash_ref.bound_sprite().ok_or_else(|| {
        ScriptError::new_code(
            crate::player::ScriptErrorCode::InvalidReference,
            "Owned Flash object has no sprite binding".to_owned(),
        )
    })?;
    checked_sprite_number(sprite_num)?;
    Ok((owner_key, generation, sprite_num))
}

fn owned_binding(player: &DirPlayer, flash_ref: &FlashObjectRef) -> Result<(String, u64, i32), ScriptError> {
    validate_owned_binding(player, flash_ref)
}

fn owned_cast_pair(
    player: &DirPlayer,
    flash_ref: &FlashObjectRef,
    generation: u64,
    sprite_num: i32,
) -> Result<(i32, i32), ScriptError> {
    if flash_ref.cast_lib != 0 || flash_ref.cast_member != 0 {
        return Ok((flash_ref.cast_lib, flash_ref.cast_member));
    }
    if player.flash_instance_generation(sprite_num as i16) != Some(generation) {
        return Err(ScriptError::new_code(
            crate::player::ScriptErrorCode::InvalidReference,
            "Flash object binding generation is stale".to_owned(),
        ));
    }
    match player.flash_binding_origin(sprite_num as i16) {
        crate::player::FlashBindingOrigin::FirstPublished { cast_lib, cast_member } => {
            Ok((cast_lib, cast_member))
        }
        _ => Err(ScriptError::new_code(
            crate::player::ScriptErrorCode::InvalidReference,
            "Flash object has no published cast binding".to_owned(),
        )),
    }
}

fn owned_player_id(player: &DirPlayer) -> Result<u32, ScriptError> {
    u32::try_from(player.owner.key().player).map_err(|_| {
        ScriptError::new_code(
            crate::player::ScriptErrorCode::InvalidReference,
            "owner player id exceeds the VM player id range".to_owned(),
        )
    })
}

pub(crate) fn checked_sprite_number(sprite_num: i32) -> Result<i16, ScriptError> {
    let sprite_num = i16::try_from(sprite_num).map_err(|_| {
        ScriptError::new_code(
            crate::player::ScriptErrorCode::InvalidReference,
            "Flash sprite number is outside the supported range".to_owned(),
        )
    })?;
    if sprite_num <= 0 {
        return Err(ScriptError::new_code(
            crate::player::ScriptErrorCode::InvalidReference,
            "Flash sprite number must be positive".to_owned(),
        ));
    }
    Ok(sprite_num)
}

fn owned_path(player: &DirPlayer, datum: &DatumRef, suffix: &str) -> Result<(String, u64, i32, i32, i32), ScriptError> {
    let flash_ref = player
        .get_datum(datum)
        .as_flash_object()
        .ok_or_else(|| ScriptError::new("Not a Flash object".to_owned()))?;
    let (_owner_key, generation, sprite_num) = owned_binding(player, flash_ref)?;
    let (cast_lib, cast_member) = owned_cast_pair(player, flash_ref, generation, sprite_num)?;
    Ok((
        format!("{}.{}", flash_ref.path, suffix),
        generation,
        sprite_num,
        cast_lib,
        cast_member,
    ))
}

fn envelope_property(value: &JsValue, name: &str) -> Result<JsValue, FlashRequestError> {
    js_sys::Reflect::get(value, &JsValue::from_str(name)).map_err(|error| {
        FlashRequestError::Script(ScriptError::new(format!(
            "Flash host response missing {name}: {error:?}"
        )))
    })
}

const MAX_FLASH_DECODE_DEPTH: usize = 32;
const MAX_FLASH_DECODE_NODES: usize = 4096;

fn decode_owned_value(value: JsValue, fallback_path: Option<&str>) -> Result<FlashDecodedValue, FlashRequestError> {
    fn decode(value: JsValue, fallback_path: Option<&str>, depth: usize, nodes: &mut usize) -> Result<FlashDecodedValue, FlashRequestError> {
        if depth > MAX_FLASH_DECODE_DEPTH || *nodes >= MAX_FLASH_DECODE_NODES {
            return Err(FlashRequestError::Script(ScriptError::new(
                "Flash host result exceeds decode limits".to_owned(),
            )));
        }
        *nodes += 1;
    if value.is_null() || value.is_undefined() {
        return Ok(FlashDecodedValue::Void);
    }
    if let Some(value) = value.as_string() {
        if value.starts_with("[object ") || value.starts_with("[type ") {
            if let Some(fallback_path) = fallback_path {
                return Ok(FlashDecodedValue::Object(fallback_path.to_owned()));
            }
        }
        return Ok(FlashDecodedValue::String(value));
    }
    if let Some(value) = value.as_f64() {
        if !value.is_finite() {
            return Err(FlashRequestError::Script(ScriptError::new(
                "Flash host returned a non-finite number".to_owned(),
            )));
        }
        if value.fract() == 0.0 && value >= i32::MIN as f64 && value <= i32::MAX as f64 {
            return Ok(FlashDecodedValue::Int(value as i32));
        }
        return Ok(FlashDecodedValue::Float(value));
    }
    if let Some(value) = value.as_bool() {
        return Ok(FlashDecodedValue::Bool(value));
    }
    if value.is_instance_of::<js_sys::Date>() {
        let timestamp = value.unchecked_into::<js_sys::Date>().get_time();
        if !timestamp.is_finite() {
            return Err(FlashRequestError::Script(ScriptError::new(
                "Flash host returned an invalid Date".to_owned(),
            )));
        }
        return Ok(FlashDecodedValue::Date(timestamp));
    }
    if value.is_object() {
        let stored_path = js_sys::Reflect::get(&value, &JsValue::from_str("__dirplayer_stored_path"))
            .map_err(|error| FlashRequestError::Script(ScriptError::new(format!(
                "Flash object path lookup failed: {error:?}"
            ))))?
            .as_string();
        if let Some(path) = stored_path {
            return Ok(FlashDecodedValue::Object(path));
        }
        if js_sys::Array::is_array(&value) {
            let array = js_sys::Array::from(&value);
            let width = array.length() as usize;
            if width > MAX_FLASH_DECODE_NODES.saturating_sub(*nodes) {
                return Err(FlashRequestError::Script(ScriptError::new(
                    "Flash host array exceeds decode limits".to_owned(),
                )));
            }
            let mut values = Vec::with_capacity(width);
            for index in 0..array.length() {
                // The request path identifies only the top-level object. It
                // must not be copied onto nested strings or untagged objects
                // inside an ordinary returned array.
                values.push(decode(array.get(index), None, depth + 1, nodes)?);
            }
            return Ok(FlashDecodedValue::List(values));
        }
        // A get/property request can identify an untagged object by its exact
        // logical path. A call result cannot: assuming `receiver.method` would
        // fabricate identity for an arbitrary returned object.
        let Some(fallback_path) = fallback_path else {
            return Err(FlashRequestError::Script(ScriptError::new(
                "Flash host returned an untagged object without identity".to_owned(),
            )));
        };
        return Ok(FlashDecodedValue::Object(fallback_path.to_owned()));
    }
    Ok(FlashDecodedValue::Void)
    }
    let mut nodes = 0;
    decode(value, fallback_path, 0, &mut nodes)
}

/// Decode the complete host envelope before any player/session borrow is
/// reacquired. This prevents JS getters/proxies from running while allocator
/// or symbol-table state is borrowed.
pub(crate) fn decode_owned_response(value: JsValue, fallback_path: Option<&str>) -> Result<FlashOwnedResponse, FlashRequestError> {
    let ok = envelope_property(&value, "ok")?
        .as_bool()
        .ok_or_else(|| FlashRequestError::Script(ScriptError::new("Flash host response has non-boolean ok".to_owned())))?;
    if !ok {
        let code = envelope_property(&value, "code")?.as_string().unwrap_or_else(|| "host-error".to_owned());
        if code == "not-ready" {
            let generation = envelope_property(&value, "generation")?
                .as_f64()
                .filter(|value| value.is_finite() && *value >= 1.0 && *value <= 9_007_199_254_740_991.0 && value.fract() == 0.0)
                .map(|value| value as u64)
                .ok_or_else(|| FlashRequestError::Script(ScriptError::new("Flash not-ready response has invalid generation".to_owned())))?;
            return Err(FlashRequestError::NotReady { generation });
        }
        let message = envelope_property(&value, "message")?.as_string();
        return Err(FlashRequestError::Script(ScriptError::new(
            message.unwrap_or_else(|| format!("Flash host operation failed: {code}")),
        )));
    }
    let generation = envelope_property(&value, "generation")?
        .as_f64()
        .filter(|value| value.is_finite() && *value >= 1.0 && *value <= 9_007_199_254_740_991.0 && value.fract() == 0.0)
        .map(|value| value as u64)
        .ok_or_else(|| FlashRequestError::Script(ScriptError::new("Flash host response has invalid generation".to_owned())))?;
    Ok(FlashOwnedResponse {
        generation,
        value: decode_owned_value(envelope_property(&value, "value")?, fallback_path)?,
    })
}

fn decode_sprite_value(
    value: JsValue,
    return_mode: &FlashReturnMode,
) -> Result<FlashDecodedValue, FlashRequestError> {
    match return_mode {
        FlashReturnMode::Object { fallback_path } => {
            let fallback_path = fallback_path.as_deref().ok_or_else(|| {
                FlashRequestError::Script(ScriptError::new(
                    "Sprite object get has no requested path".to_owned(),
                ))
            })?;
            if let Some(value) = value.as_string() {
                if value.starts_with("[object ") || value.starts_with("[type ") {
                    return Ok(FlashDecodedValue::Object(fallback_path.to_owned()));
                }
                return Ok(FlashDecodedValue::String(value));
            }
            if value.is_object() {
                if let Some(path) = js_sys::Reflect::get(
                    &value,
                    &JsValue::from_str("__dirplayer_stored_path"),
                )
                .ok()
                .and_then(|value| value.as_string())
                .filter(|path| is_canonical_stored_flash_path(path))
                {
                    return Ok(FlashDecodedValue::Object(path));
                }
            }
            Ok(FlashDecodedValue::Object(fallback_path.to_owned()))
        }
        FlashReturnMode::Scalar => {
            if value.is_null() || value.is_undefined() {
                return Ok(FlashDecodedValue::Void);
            }
            if let Some(value) = value.as_string() {
                return Ok(FlashDecodedValue::String(value));
            }
            if let Some(value) = value.as_bool() {
                return Ok(FlashDecodedValue::Int(if value { 1 } else { 0 }));
            }
            if let Some(value) = value.as_f64() {
                if !value.is_finite() {
                    return Err(FlashRequestError::Script(ScriptError::new(
                        "Flash host returned a non-finite sprite value".to_owned(),
                    )));
                }
                if value.fract() == 0.0 && value.abs() < i32::MAX as f64 {
                    return Ok(FlashDecodedValue::Int(value as i32));
                }
                return Ok(FlashDecodedValue::Float(value));
            }
            Ok(FlashDecodedValue::Void)
        }
    }
}

#[cfg(test)]
mod sprite_decode_tests {
    use super::is_canonical_stored_flash_path;

    #[test]
    fn stored_flash_path_shape_is_bounded_and_canonical() {
        assert!(is_canonical_stored_flash_path("_level0.__dirplayer_ref_1"));
        assert!(is_canonical_stored_flash_path("_level0.__dirplayer_ref_4294967295"));
        assert!(!is_canonical_stored_flash_path("_level0.__dirplayer_ref_0x1"));
        assert!(!is_canonical_stored_flash_path("_level0.__dirplayer_ref_4294967296"));
        assert!(!is_canonical_stored_flash_path("_root.__dirplayer_ref_1"));
    }
}

fn decode_sprite_response(
    value: JsValue,
    return_mode: &FlashReturnMode,
) -> Result<FlashOwnedResponse, FlashRequestError> {
    let ok = envelope_property(&value, "ok")?
        .as_bool()
        .ok_or_else(|| FlashRequestError::Script(ScriptError::new(
            "Flash host response has non-boolean ok".to_owned(),
        )))?;
    if !ok {
        let code = envelope_property(&value, "code")?
            .as_string()
            .unwrap_or_else(|| "host-error".to_owned());
        if code == "not-ready" {
            let generation = envelope_property(&value, "generation")?
                .as_f64()
                .filter(|value| {
                    value.is_finite()
                        && *value >= 1.0
                        && *value <= 9_007_199_254_740_991.0
                        && value.fract() == 0.0
                })
                .map(|value| value as u64)
                .ok_or_else(|| {
                    FlashRequestError::Script(ScriptError::new(
                        "Flash not-ready response has invalid generation".to_owned(),
                    ))
                })?;
            return Err(FlashRequestError::NotReady { generation });
        }
        let message = envelope_property(&value, "message")?.as_string();
        return Err(FlashRequestError::Script(ScriptError::new(
            message.unwrap_or_else(|| format!("Flash host operation failed: {code}")),
        )));
    }
    let generation = envelope_property(&value, "generation")?
        .as_f64()
        .filter(|value| {
            value.is_finite()
                && *value >= 1.0
                && *value <= 9_007_199_254_740_991.0
                && value.fract() == 0.0
        })
        .map(|value| value as u64)
        .ok_or_else(|| {
            FlashRequestError::Script(ScriptError::new(
                "Flash host response has invalid generation".to_owned(),
            ))
        })?;
    Ok(FlashOwnedResponse {
        generation,
        value: decode_sprite_value(envelope_property(&value, "value")?, return_mode)?,
    })
}

pub(crate) fn execute_flash_request(request: &FlashRequest) -> Result<FlashOwnedResponse, FlashRequestError> {
    let owner_key = owner_key_string(&request.owner);
    let generation_as_number = |generation: u64| {
        if generation == 0 || generation > 9_007_199_254_740_991 {
            Err(FlashRequestError::Script(ScriptError::new(
                "Flash instance generation exceeds JavaScript safe integer range".to_owned(),
            )))
        } else {
            Ok(generation as f64)
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = request;
        return Err(FlashRequestError::Script(ScriptError::new(
            "Flash host is unavailable on native".to_owned(),
        )));
    }
    #[cfg(target_arch = "wasm32")]
    {
        let response = match (&request.operation, request.expected_generation) {
            (FlashOperation::BindGet { return_mode }, None) => ruffle_get_variable_owned_for_binding(
                &owner_key, request.sprite_num, &request.path, return_mode.is_object(),
            ),
            (FlashOperation::Get { return_mode }, Some(generation))
            | (FlashOperation::BindGet { return_mode }, Some(generation)) => ruffle_get_variable_owned_at_generation(
                &owner_key, request.sprite_num, generation_as_number(generation)?, &request.path,
                return_mode.is_object(),
            ),
            (FlashOperation::SpriteGet { .. }, Some(generation)) =>
                ruffle_get_sprite_variable_owned_at_generation(
                    &owner_key,
                    request.sprite_num,
                    generation_as_number(generation)?,
                    &request.path,
                ),
            (FlashOperation::Set { value }, Some(generation)) => ruffle_set_variable_owned_at_generation(
                &owner_key, request.sprite_num, generation_as_number(generation)?, &request.path, value,
            ),
            (FlashOperation::Call { args_xml }, Some(generation)) => ruffle_call_function_owned_at_generation(
                &owner_key, request.sprite_num, generation_as_number(generation)?, &request.path, args_xml,
            ),
            _ => return Err(FlashRequestError::Script(ScriptError::new(
                "owned Flash request has an invalid operation/generation pair".to_owned(),
            ))),
        }.map_err(|error| FlashRequestError::Script(ScriptError::new(format!(
            "Flash host invocation failed: {error:?}"
        ))))?;
        let fallback_path = match &request.operation {
            FlashOperation::BindGet { return_mode }
            | FlashOperation::Get { return_mode }
            | FlashOperation::SpriteGet { return_mode } => {
                return_mode.fallback_path(&request.path)
            }
            FlashOperation::Set { .. } | FlashOperation::Call { .. } => None,
        };
        match &request.operation {
            FlashOperation::SpriteGet { return_mode } => {
                decode_sprite_response(response, return_mode)
            }
            _ => decode_owned_response(response, fallback_path),
        }
    }
}

pub(crate) async fn wait_for_flash_ready_owned(
    owner: &OwnerToken,
    sprite_num: i32,
    generation: u64,
) -> Result<(), ScriptError> {
    let sprite_num = checked_sprite_number(sprite_num)?;
    let generation_u64 = if generation == 0 || generation > 9_007_199_254_740_991 {
        return Err(ScriptError::new("Flash instance generation is not a safe integer".to_owned()));
    } else {
        generation
    };
    let generation_js = generation_u64 as f64;
    let owner_key = owner_key_string(owner);
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (owner_key, sprite_num, generation_js);
        return Err(ScriptError::new("Flash readiness is unavailable on native".to_owned()));
    }
    #[cfg(target_arch = "wasm32")]
    {
        for _ in 0..100u32 {
            if !owner.is_arena_live() {
                return Err(ScriptError::new("Flash readiness wait was cancelled".to_owned()));
            }
            let response = is_flash_instance_ready_owned(&owner_key, sprite_num as i32, generation_js)
                .map_err(|error| ScriptError::new(format!("Flash readiness query failed: {error:?}")))?;
            let ok = js_sys::Reflect::get(&response, &JsValue::from_str("ok"))
                .map_err(|error| ScriptError::new(format!("Flash readiness response ok lookup failed: {error:?}")))?
                .as_bool()
                .ok_or_else(|| ScriptError::new("Flash readiness response has non-boolean ok".to_owned()))?;
            if !ok {
                let code = js_sys::Reflect::get(&response, &JsValue::from_str("code"))
                    .ok()
                    .and_then(|value| value.as_string())
                    .unwrap_or_else(|| "host-error".to_owned());
                if code == "missing-instance" {
                    let response_generation = js_sys::Reflect::get(
                        &response,
                        &JsValue::from_str("generation"),
                    )
                    .map_err(|error| {
                        ScriptError::new(format!(
                            "Flash readiness response generation lookup failed: {error:?}"
                        ))
                    })?;
                    if !response_generation.is_undefined() {
                        let response_generation = response_generation
                            .as_f64()
                            .filter(|value| {
                                value.is_finite()
                                    && *value >= 1.0
                                    && *value <= 9_007_199_254_740_991.0
                                    && value.fract() == 0.0
                            })
                            .map(|value| value as u64)
                            .ok_or_else(|| {
                                ScriptError::new(
                                    "Flash readiness response has invalid generation".to_owned(),
                                )
                            })?;
                        if response_generation != generation_u64 {
                            return Err(ScriptError::new("Flash readiness generation changed".to_owned()));
                        }
                    }
                    if !owner.is_arena_live() {
                        return Err(ScriptError::new("Flash readiness wait was cancelled".to_owned()));
                    }
                    async_std::task::sleep(std::time::Duration::from_millis(100)).await;
                    continue;
                }
                return Err(ScriptError::new(format!("Flash readiness rejected: {code}")));
            }
            if !owner.is_arena_live() {
                return Err(ScriptError::new("Flash readiness wait was cancelled".to_owned()));
            }
            let response_generation = js_sys::Reflect::get(
                &response,
                &JsValue::from_str("generation"),
            )
            .map_err(|error| {
                ScriptError::new(format!(
                    "Flash readiness response generation lookup failed: {error:?}"
                ))
            })?
            .as_f64()
            .filter(|value| {
                value.is_finite()
                    && *value >= 1.0
                    && *value <= 9_007_199_254_740_991.0
                    && value.fract() == 0.0
            })
            .map(|value| value as u64)
            .ok_or_else(|| {
                ScriptError::new("Flash readiness response has invalid generation".to_owned())
            })?;
            if response_generation != generation_u64 {
                return Err(ScriptError::new("Flash readiness generation changed".to_owned()));
            }
            let ready = js_sys::Reflect::get(&response, &JsValue::from_str("ready"))
                .ok()
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            if !owner.is_arena_live() {
                return Err(ScriptError::new("Flash readiness wait was cancelled".to_owned()));
            }
            if ready {
                return Ok(());
            }
            async_std::task::sleep(std::time::Duration::from_millis(100)).await;
        }
        Err(ScriptError::new("Flash instance did not become ready".to_owned()))
    }
}

pub struct FlashObjectDatumHandlers {}

/// Compatibility name retained for the existing SetObjProp continuation.
/// The value is now a fully typed owner/generation-bound Flash request.
pub type FlashSetPropertyRequest = FlashRequest;

pub fn prepare_set_prop(
    player: &DirPlayer,
    symbols: &SymbolTable,
    datum: &DatumRef,
    prop_name: Symbol,
    value: &Datum,
) -> Result<FlashSetPropertyRequest, ScriptError> {
    let flash_ref = player
        .allocator
        .try_get_datum(datum)
        .and_then(Datum::as_flash_object)
        .cloned()
        .ok_or_else(|| ScriptError::new("Not a Flash object".to_string()))?;
    let prop_name = symbols
        .display(&prop_name)
        .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
        .to_owned();
    let (_owner_key, generation, sprite_num) = owned_binding(player, &flash_ref)?;
    let (cast_lib, cast_member) = owned_cast_pair(player, &flash_ref, generation, sprite_num)?;
    let value = match value {
        Datum::Int(i) => i.to_string(),
        Datum::Float(f) => f.to_string(),
        Datum::String(s) => s.clone(),
        Datum::Void => "null".to_string(),
        _ => "null".to_string(),
    };
    Ok(FlashSetPropertyRequest {
        player_id: owned_player_id(player)?,
        owner: player.owner.clone(),
        sprite_num,
        expected_generation: Some(generation),
        path: format!("{}.{}", flash_ref.path, prop_name),
        operation: FlashOperation::Set { value },
        cast_lib,
        cast_member,
    })
}

impl FlashObjectDatumHandlers {
    pub fn prepare_global_bind_get(
        player: &DirPlayer,
        symbols: &SymbolTable,
        args: &[DatumRef],
    ) -> Result<Option<FlashRequest>, ScriptError> {
        let Some(sprite_ref) = args.first() else { return Ok(None) };
        let sprite_num = match player.allocator.try_get_datum(sprite_ref) {
            Some(Datum::SpriteRef(sprite_num)) => checked_sprite_number(*sprite_num as i32)?,
            Some(Datum::Int(sprite_num)) => checked_sprite_number(*sprite_num)?,
            Some(Datum::FlashObjectRef(_)) => {
                return Err(ScriptError::new_code(
                    crate::player::ScriptErrorCode::InvalidReference,
                    "getVariable sprite argument cannot be a Flash object reference".to_owned(),
                ));
            }
            None => {
                return Err(ScriptError::new_code(
                    crate::player::ScriptErrorCode::InvalidReference,
                    "invalid getVariable sprite reference".to_owned(),
                ));
            }
            _ => return Ok(None),
        };
        let Some(path_ref) = args.get(1) else { return Ok(None) };
        let path = player
            .allocator
            .try_get_datum(path_ref)
            .ok_or_else(|| ScriptError::new_code(
                crate::player::ScriptErrorCode::InvalidReference,
                "invalid getVariable path reference".to_owned(),
            ))?
            .string_value(symbols)?;
        let return_as_object = match args.get(2) {
            Some(flag) => player
                .allocator
                .try_get_datum(flag)
                .ok_or_else(|| ScriptError::new_code(
                    crate::player::ScriptErrorCode::InvalidReference,
                    "invalid getVariable return-mode reference".to_owned(),
                ))?
                .int_value()
                .unwrap_or(1)
                == 0,
            None => false,
        };
        let (cast_lib, cast_member) = player
            .movie
            .score
            .get_sprite(sprite_num)
            .and_then(|sprite| sprite.member.as_ref())
            .map(|member| (member.cast_lib, member.cast_member))
            .unwrap_or((0, 0));
        Ok(Some(Self::prepare_bind_get(
            player,
            sprite_num,
            crate::player::handlers::datum_handlers::sprite::root_flash_path(&path),
            return_as_object,
            cast_lib,
            cast_member,
        )?))
    }

    pub fn prepare_bind_get(
        player: &DirPlayer,
        sprite_num: i16,
        path: String,
        return_as_object: bool,
        cast_lib: i32,
        cast_member: i32,
    ) -> Result<FlashRequest, ScriptError> {
        checked_sprite_number(sprite_num as i32)?;
        Ok(FlashRequest {
            player_id: owned_player_id(player)?,
            owner: player.owner.clone(),
            sprite_num: sprite_num as i32,
            expected_generation: None,
            path: path.clone(),
            operation: FlashOperation::BindGet {
                return_mode: if return_as_object {
                    FlashReturnMode::Object { fallback_path: Some(path) }
                } else {
                    FlashReturnMode::Scalar
                },
            },
            cast_lib,
            cast_member,
        })
    }

    pub fn get_prop(obj_ref: &DatumRef, prop_name: &str) -> Result<DatumRef, ScriptError> {
        let _ = (obj_ref, prop_name);
        Err(ScriptError::new(
            "Flash property access requires an owned deferred request".to_owned(),
        ))
    }

    pub fn prepare_get_prop(
        player: &DirPlayer,
        datum: &DatumRef,
        prop_name: &str,
    ) -> Result<FlashRequest, ScriptError> {
        let (path, generation, sprite_num, cast_lib, cast_member) =
            owned_path(player, datum, prop_name)?;
        Ok(FlashRequest {
            player_id: owned_player_id(player)?,
            owner: player.owner.clone(),
            sprite_num,
            expected_generation: Some(generation),
            path: path.clone(),
            operation: FlashOperation::Get {
                return_mode: FlashReturnMode::Object {
                    fallback_path: Some(path.clone()),
                },
            },
            cast_lib,
            cast_member,
        })
    }

    pub fn prepare_call(
        player: &DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        handler_name: Symbol,
        args: &[DatumRef],
    ) -> Result<FlashRequest, ScriptError> {
        let flash_ref = player
            .allocator
            .try_get_datum(datum)
            .and_then(Datum::as_flash_object)
            .cloned()
            .ok_or_else(|| ScriptError::new("Not a Flash object".to_owned()))?;
        let (_owner_key, generation, sprite_num) = owned_binding(player, &flash_ref)?;
        let (cast_lib, cast_member) = owned_cast_pair(player, &flash_ref, generation, sprite_num)?;
        let handler_name = symbols
            .display(&handler_name)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        let mut nodes = 0;
        let mut active = Vec::new();
        let mut encoded = Vec::with_capacity(args.len());
        for arg in args {
            encoded.push(convert_lingo_datum_to_json_ref(
                player,
                symbols,
                arg,
                sprite_num,
                0,
                &mut nodes,
                &mut active,
            )?);
        }
        let args_xml = format!("[{}]", encoded.join(","));
        Ok(FlashRequest {
            player_id: owned_player_id(player)?,
            owner: player.owner.clone(),
            sprite_num,
            expected_generation: Some(generation),
            path: format!("{}.{}", flash_ref.path, handler_name),
            operation: FlashOperation::Call { args_xml },
            cast_lib,
            cast_member,
        })
    }

    pub(crate) fn prepare_sprite_call(
        player: &DirPlayer,
        sprite_num: i16,
        generation: u64,
        path: String,
        args_xml: String,
        cast_lib: i32,
        cast_member: i32,
    ) -> Result<FlashRequest, ScriptError> {
        checked_sprite_number(sprite_num as i32)?;
        if generation == 0 || generation > 9_007_199_254_740_991 {
            return Err(ScriptError::new_code(
                crate::player::ScriptErrorCode::InvalidReference,
                "Flash sprite call has an invalid or unsafe instance generation".to_owned(),
            ));
        }
        Ok(FlashRequest {
            player_id: owned_player_id(player)?,
            owner: player.owner.clone(),
            sprite_num: sprite_num as i32,
            expected_generation: Some(generation),
            path,
            operation: FlashOperation::Call { args_xml },
            cast_lib,
            cast_member,
        })
    }

    pub fn call(
        _symbols: &mut SymbolTable,
        _datum: &DatumRef,
        _handler_name: Symbol,
        _args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        Err(ScriptError::new(
            "Flash method calls require an owned deferred request".to_owned(),
        ))
    }


}

pub(crate) fn decoded_to_datum(
    player: &mut DirPlayer,
    response: FlashOwnedResponse,
    request: &FlashRequest,
) -> Result<DatumRef, ScriptError> {
    if request.expected_generation != Some(response.generation) {
        return Err(ScriptError::new_code(
            crate::player::ScriptErrorCode::InvalidReference,
            "Flash response generation no longer matches request".to_owned(),
        ));
    }
    let owner_key = owner_key_string(&request.owner);
    fn convert(
        player: &mut DirPlayer,
        value: FlashDecodedValue,
        owner_key: &str,
        generation: u64,
        cast_lib: i32,
        cast_member: i32,
        sprite_num: i32,
    ) -> Result<DatumRef, ScriptError> {
        Ok(match value {
            FlashDecodedValue::Void => player.alloc_datum(Datum::Void),
            FlashDecodedValue::String(value) => player.alloc_datum(Datum::String(value)),
            FlashDecodedValue::Int(value) => player.alloc_datum(Datum::Int(value)),
            FlashDecodedValue::Float(value) => player.alloc_datum(Datum::Float(value)),
            FlashDecodedValue::Bool(value) => player.alloc_datum(Datum::Int(i32::from(value))),
            FlashDecodedValue::Date(value) => {
                let id = player.allocator.get_free_script_instance_id();
                player.date_objects.insert(id, DateObject::from_timestamp(id, value as i64));
                player.alloc_datum(Datum::DateRef(id))
            }
            FlashDecodedValue::Object(path) => player.alloc_datum(Datum::FlashObjectRef(
                FlashObjectRef::from_path_with_sprite(
                    &path,
                    cast_lib,
                    cast_member,
                    sprite_num,
                )
                    .with_binding(owner_key.to_owned(), generation),
            )),
            FlashDecodedValue::List(values) => {
                let mut items = VecDeque::with_capacity(values.len());
                for value in values {
                    items.push_back(convert(
                        player,
                        value,
                        owner_key,
                        generation,
                        cast_lib,
                        cast_member,
                        sprite_num,
                    )?);
                }
                player.alloc_datum(Datum::List(
                    crate::director::lingo::datum::DatumType::XmlChildNodes,
                    items,
                    false,
                ))
            }
        })
    }
    convert(
        player,
        response.value,
        &owner_key,
        response.generation,
        request.cast_lib,
        request.cast_member,
        request.sprite_num,
    )
}

const MAX_FLASH_ARGUMENT_DEPTH: usize = 32;
const MAX_FLASH_ARGUMENT_NODES: usize = 4096;
const MAX_FLASH_ARGUMENT_WIDTH: usize = 4096;

fn convert_lingo_datum_to_json_ref(
    player: &crate::player::DirPlayer,
    symbols: &SymbolTable,
    datum_ref: &DatumRef,
    target_sprite: i32,
    depth: usize,
    nodes: &mut usize,
    active: &mut Vec<usize>,
) -> Result<String, ScriptError> {
    if depth > MAX_FLASH_ARGUMENT_DEPTH || *nodes >= MAX_FLASH_ARGUMENT_NODES {
        return Err(ScriptError::new("Flash argument graph exceeds limits".to_owned()));
    }
    *nodes += 1;
    if matches!(datum_ref, DatumRef::Void) {
        return Ok("0".to_owned());
    }
    let id = datum_ref.unwrap();
    if active.contains(&id) {
        return Err(ScriptError::new("cyclic Flash argument graph".to_owned()));
    }
    active.push(id);
    let result = player
        .allocator
        .try_get_datum(datum_ref)
        .ok_or_else(|| ScriptError::new_code(
            crate::player::ScriptErrorCode::InvalidReference,
            format!("invalid Flash argument reference {datum_ref}"),
        ))
        .and_then(|datum| {
            convert_lingo_datum_to_json_inner(
                player,
                symbols,
                datum,
                target_sprite,
                depth,
                nodes,
                active,
            )
        });
    active.pop();
    result
}

fn convert_lingo_datum_to_json_inner(
    player: &crate::player::DirPlayer,
    symbols: &SymbolTable,
    datum: &Datum,
    target_sprite: i32,
    depth: usize,
    nodes: &mut usize,
    active: &mut Vec<usize>,
) -> Result<String, ScriptError> {
    match datum {
        Datum::Int(i) => Ok(i.to_string()),
        Datum::Float(f) if f.is_finite() => Ok(f.to_string()),
        Datum::Float(_) => Err(ScriptError::new("non-finite Flash argument".to_owned())),
        Datum::String(s) => {
            serde_json::to_string(s).map_err(|error| ScriptError::new(format!("invalid Flash string argument: {error}")))
        },
        Datum::Symbol(s) => {
            let value = format!("#{}", symbols.display(s).map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?);
            serde_json::to_string(&value).map_err(|error| ScriptError::new(format!("invalid Flash symbol argument: {error}")))
        },
        // Match Adobe's Flash Asset Xtra: Lingo Void crosses into AS as the
        // numeric value 0, not null. CS's outgoing AV packets need this:
        //   - dance/stand send `(VOID, VOID, VOID, VOID, "dnc")` and the
        //     server requires `d="0"` — getUpdateAvatarNode gates `d` on
        //     `iDirection != null && != "" && toString() != "NaN"`. Null gets
        //     dropped, but 0 passes (0 != null is true, 0 != "" is true under
        //     Ruffle's equality, "0" != "NaN").
        //   - faceAvatar sends `(VOID, VOID, VOID, iDir, VOID)`. With number 0
        //     for the action slot the `typeof sAction == "string"` gate fails,
        //     so we don't pollute the packet with `act="0"`.
        //   - position args have an extra `!= 0` gate that filters 0 out, so
        //     x/y/z stay omitted when no movement happened.
        Datum::Void => Ok("0".to_string()),
        Datum::FlashObjectRef(flash_ref) => {
            let current_owner = owner_key_string(&player.owner);
            let Some(sprite_num) = flash_ref.bound_sprite() else {
                return Err(ScriptError::new_code(
                    crate::player::ScriptErrorCode::InvalidReference,
                    "Flash object argument has no sprite binding".to_owned(),
                ));
            };
            let Some(generation) = flash_ref.instance_generation else {
                return Err(ScriptError::new_code(
                    crate::player::ScriptErrorCode::InvalidReference,
                    "Flash object argument has no instance generation".to_owned(),
                ));
            };
            let sprite_num = checked_sprite_number(sprite_num as i32)? as i32;
            if flash_ref.owner_key.as_deref() != Some(current_owner.as_str())
                || sprite_num != target_sprite
                || !player.is_flash_instance_generation_current(sprite_num as i16, generation)
            {
                return Err(ScriptError::new_code(
                    crate::player::ScriptErrorCode::InvalidReference,
                    "foreign or stale Flash object argument".to_owned(),
                ));
            }
            let path = format!("__ruffle_path:{}", flash_ref.path);
            serde_json::to_string(&path).map_err(|error| ScriptError::new(format!("invalid Flash path argument: {error}")))
        },
        Datum::List(_, items, _) => {
            if items.len() > MAX_FLASH_ARGUMENT_WIDTH {
                return Err(ScriptError::new("Flash argument list exceeds width limit".to_owned()));
            }
            let mut parts = Vec::with_capacity(items.len());
            for item_ref in items {
                parts.push(convert_lingo_datum_to_json_ref(
                    player,
                    symbols,
                    item_ref,
                    target_sprite,
                    depth + 1,
                    nodes,
                    active,
                )?);
            }
            Ok(format!("[{}]", parts.join(",")))
        },
        _ => Ok("null".to_string()),
    }
}

fn convert_js_result_to_lingo_datum(
    player: &mut crate::player::DirPlayer,
    result: JsValue,
    context_path: &str,
    cast_lib: i32,
    cast_member: i32,
) -> Result<DatumRef, ScriptError> {
    if result.is_null() || result.is_undefined() {
        return Ok(player.alloc_datum(Datum::Void));
    }

    if let Some(s) = result.as_string() {
        return Ok(player.alloc_datum(Datum::String(s)));
    }

    if let Some(n) = result.as_f64() {
        if !n.is_finite() {
            return Err(ScriptError::new("Flash host returned a non-finite number".to_owned()));
        }
        if n.fract() == 0.0 && n >= i32::MIN as f64 && n <= i32::MAX as f64 {
            return Ok(player.alloc_datum(Datum::Int(n as i32)));
        } else {
            return Ok(player.alloc_datum(Datum::Float(n)));
        }
    }

    if let Some(b) = result.as_bool() {
        return Ok(player.alloc_datum(Datum::Int(if b { 1 } else { 0 })));
    }

    // AS Date instances arrive as JS Date — materialize as a Lingo DateRef
    // so subsequent .getYear()/.getMonth()/.getDate() etc. dispatch through
    // DateDatumHandlers (case-insensitive) instead of round-tripping back into
    // Ruffle (case-sensitive AS, which would fail).
    if result.is_instance_of::<js_sys::Date>() {
        let date: js_sys::Date = result.unchecked_into();
        let timestamp = date.get_time();
        if !timestamp.is_finite() {
            return Err(ScriptError::new("Flash host returned an invalid Date".to_owned()));
        }
        let ts = timestamp as i64;
        let date_id = player.allocator.get_free_script_instance_id();
        let date_obj = DateObject::from_timestamp(date_id, ts);
        player.date_objects.insert(date_id, date_obj);
        return Ok(player.alloc_datum(Datum::DateRef(date_id)));
    }

    // Check for arrays before generic objects (arrays are objects in JS).
    //
    // BUT: if the array is marked with `__dirplayer_stored_path` it means
    // our Ruffle fork is keeping a live AS reference for it — typically
    // because Lingo intends to push into it and pass it back to AS
    // (e.g. Coke Studios' Studio.sendSendMessage flow:
    //   faToScreenNameList = me.foRoom.getArray()
    //   faToScreenNameList.push(name)
    //   me.foRoom.sendSendMessage(msg, faToScreenNameList)).
    // Materializing it as a Lingo Datum::List severs the AS reference, so
    // .push() mutates a local list and the AS array stays empty. Keep it as
    // a FlashObjectRef instead so .push round-trips back to AS.
    if js_sys::Array::is_array(&result) {
        let stored_path = js_sys::Reflect::get(&result, &JsValue::from_str("__dirplayer_stored_path"))
            .ok()
            .and_then(|v| v.as_string());
        if let Some(path) = stored_path {
            let flash_ref = FlashObjectRef::from_path_with_member(&path, cast_lib, cast_member);
            return Ok(player.alloc_datum(Datum::FlashObjectRef(flash_ref)));
        }
        let array = js_sys::Array::from(&result);
        let mut items = VecDeque::new();
        for i in 0..array.length() {
            let item = array.get(i);
            let item_ref = convert_js_result_to_lingo_datum(player, item, context_path, cast_lib, cast_member)?;
            items.push_back(item_ref);
        }
        return Ok(player.alloc_datum(Datum::List(
            crate::director::lingo::datum::DatumType::XmlChildNodes, // 0-based indexing for Flash arrays
            items,
            false,
        )));
    }

    if result.is_object() {
        // Check if Ruffle stored the object and included the path
        let stored_path = js_sys::Reflect::get(&result, &JsValue::from_str("__dirplayer_stored_path"))
            .ok()
            .and_then(|v| v.as_string());

        if let Some(path) = stored_path {
            let flash_ref = FlashObjectRef::from_path_with_member(&path, cast_lib, cast_member);
            return Ok(player.alloc_datum(Datum::FlashObjectRef(flash_ref)));
        }

        // Fallback: generate a path (won't be resolvable in Flash)
        let instance_id = player.next_flash_object_id()?;
        let object_path = format!("_level0.__dirplayer_ref_{}", instance_id);
        warn!("FlashObject: no stored path, using fallback {}", object_path);
        let flash_ref = FlashObjectRef::from_path_with_member(&object_path, cast_lib, cast_member);
        return Ok(player.alloc_datum(Datum::FlashObjectRef(flash_ref)));
    }

    Ok(player.alloc_datum(Datum::Void))
}
