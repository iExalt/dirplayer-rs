// Glue between the JS-Lingo interpreter and the rest of dirplayer.
//
// Loaded scripts: `register_js_script` is called by the owner-aware cast
// application boundary for any script whose Lscr literal-data area starts
// with the SpiderMonkey XDR magic. It decodes the script, instantiates a
// JsRuntime with the stdlib + a player-bound bridge, runs the program once to
// hoist globals, and stores the runtime in the owning RuntimeSession.
//
// Handler dispatch: `try_invoke_js_handler` is the hook the existing
// `player_call_script_handler` consults before walking Lingo bytecode.
// If the named handler exists as a JS function value on the script's
// runtime global, we invoke it and convert the return value back to a
// DatumRef.
//
// Runtime and retained-object state are session-owned. Legacy setup callers
// enter through the same owner-bound registry and never consult ambient state.
//
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};

use crate::director::lingo::datum::Datum;
use crate::player::cast_lib::CastMemberRef;
use crate::player::allocator::DatumAllocatorTrait;
use crate::player::datum_ref::DatumRef;
use crate::player::js_lingo::host_bridge::{JsHostBridge, StubBridge};
use crate::player::js_lingo::value::{JsError, JsObjectRef, JsValue};
use crate::player::js_lingo::{decode_script, disasm::disassemble, JsScriptIR};
use crate::player::js_lingo::interpreter::JsRuntime;
use crate::player::compare::validate_direct_symbol_fields;
use crate::player::script::Script;
use crate::player::symbols::{builtin::BuiltInSymbol, symbol::Symbol};

/// An opaque owner-bound capability for one callable JS object. The numeric
/// coordinate is never authoritative without the captured owner identity.
#[derive(Clone, Debug)]
pub struct JsObjectHandle {
    id: u32,
    owner: crate::player::ownership::OwnerToken,
}

impl JsObjectHandle {
    fn new(id: u32, owner: crate::player::ownership::OwnerToken) -> Self {
        Self { id, owner }
    }

    pub(crate) fn id(&self) -> u32 {
        self.id
    }

    pub(crate) fn is_live_for(&self, owner: &crate::player::ownership::OwnerToken) -> bool {
        self.owner.same_identity(owner) && self.owner.is_arena_live() && owner.is_arena_live()
    }
}

impl PartialEq for JsObjectHandle {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.owner.same_identity(&other.owner)
    }
}

impl Eq for JsObjectHandle {}

/// Runtime and live-object state owned by one `RuntimeSession`. Numeric JS
/// object handles are scoped by player and owner capability; they are not
/// process-global authorities.
#[derive(Default)]
pub(crate) struct JsRuntimeRegistry {
    runtimes: HashMap<(crate::player::session::PlayerId, CastMemberRef), (crate::player::ownership::OwnerToken, Rc<RefCell<JsRuntime>>)>,
    objects: HashMap<u32, JsObjectEntry>,
    next_object_id: u32,
}

struct JsObjectEntry {
    player_id: crate::player::session::PlayerId,
    owner: crate::player::ownership::OwnerToken,
    runtime: Rc<RefCell<JsRuntime>>,
    object: JsObjectRef,
}

impl JsRuntimeRegistry {
    pub(crate) fn insert_runtime(
        &mut self,
        player_id: crate::player::session::PlayerId,
        owner: crate::player::ownership::OwnerToken,
        member_ref: CastMemberRef,
        runtime: Rc<RefCell<JsRuntime>>,
    ) {
        self.runtimes.insert((player_id, member_ref), (owner, runtime));
    }

    pub(crate) fn runtime(
        &self,
        player_id: crate::player::session::PlayerId,
        owner: &crate::player::ownership::OwnerToken,
        member_ref: &CastMemberRef,
    ) -> Option<Rc<RefCell<JsRuntime>>> {
        let (entry_owner, runtime) = self.runtimes.get(&(player_id, member_ref.clone()))?;
        (entry_owner.same_identity(owner) && owner.is_arena_live()).then(|| runtime.clone())
    }

    pub(crate) fn register_object(
        &mut self,
        player_id: crate::player::session::PlayerId,
        owner: &crate::player::ownership::OwnerToken,
        runtime: &Rc<RefCell<JsRuntime>>,
        object: &JsObjectRef,
    ) -> Result<JsObjectHandle, JsError> {
        if !owner.is_arena_live() {
            return Err(JsError::new("stale or foreign Director owner"));
        }
        if let Some((id, entry)) = self.objects.iter().find(|(_, entry)| {
            entry.player_id == player_id
                && entry.owner.same_identity(owner)
                && Rc::ptr_eq(&entry.object, object)
        }) {
            return Ok(JsObjectHandle::new(*id, owner.clone()));
        }
        let id = self
            .next_object_id
            .checked_add(1)
            .filter(|candidate| *candidate != 0)
            .ok_or_else(|| JsError::new("JS object handle space exhausted"))?;
        if self.objects.contains_key(&id) {
            return Err(JsError::new("JS object handle allocator collision"));
        }
        self.next_object_id = id;
        self.objects.insert(id, JsObjectEntry {
            player_id,
            owner: owner.clone(),
            runtime: runtime.clone(),
            object: object.clone(),
        });
        Ok(JsObjectHandle::new(id, owner.clone()))
    }

    pub(crate) fn object(
        &self,
        player_id: crate::player::session::PlayerId,
        owner: &crate::player::ownership::OwnerToken,
        handle: &JsObjectHandle,
    ) -> Option<(Rc<RefCell<JsRuntime>>, JsObjectRef)> {
        if !handle.is_live_for(owner) {
            return None;
        }
        let entry = self.objects.get(&handle.id())?;
        (entry.player_id == player_id && entry.owner.same_identity(owner))
            .then(|| (entry.runtime.clone(), entry.object.clone()))
    }

    pub(crate) fn clear(&mut self) {
        self.runtimes.clear();
        self.objects.clear();
        self.next_object_id = 0;
    }

    pub(crate) fn clear_player(
        &mut self,
        player_id: crate::player::session::PlayerId,
        owner: &crate::player::ownership::OwnerToken,
    ) {
        self.runtimes.retain(|(id, _), (entry_owner, _)| {
            !(*id == player_id && entry_owner.same_identity(owner))
        });
        self.objects.retain(|_, entry| {
            !(entry.player_id == player_id && entry.owner.same_identity(owner))
        });
        if self.objects.is_empty() {
            self.next_object_id = 0;
        }
    }
}

/// Explicit conversion state for values crossing one executing JS runtime.
/// The registry and identity are borrowed from the owning RuntimeSession;
/// there is deliberately no current-runtime global.
struct JsConversionContext<'a> {
    registry: &'a mut JsRuntimeRegistry,
    runtime: &'a Rc<RefCell<JsRuntime>>,
    session: Weak<RefCell<crate::player::session::RuntimeSession>>,
    player_id: crate::player::session::PlayerId,
    owner: crate::player::ownership::OwnerToken,
    active_arrays: HashSet<usize>,
    active_objects: HashSet<usize>,
}

/// Whether an object must cross the bridge as a live handle rather than a
/// `PropList` copy: it does as soon as anything on it is callable, since a
/// copy would flatten those methods into strings. Plain data objects keep
/// the historical PropList conversion so existing JS→Lingo records (and the
/// Lingo code that walks them with `getAt`/`getPropAt`) are unaffected.
fn object_has_callable(obj: &JsObjectRef) -> bool {
    let mut current = Some(obj.clone());
    // Bounded walk: a malformed proto cycle must not hang the player.
    for _ in 0..32 {
        let Some(o) = current else { return false };
        let b = o.borrow();
        if b.props.iter().any(|(_, v)| matches!(v, JsValue::Function(_) | JsValue::Native(_))) {
            return true;
        }
        current = b.proto.clone();
    }
    false
}

/// Invoke a method on a live JS object on behalf of Lingo. `name` is matched
/// case-sensitively first, then case-insensitively — Lingo symbols don't
/// preserve the case JS declared the property with.
///
/// Returns `None` when the object has no callable property of that name, so
/// the caller can raise Lingo's own "no handler" error.
/// Look a property up on a live JS object, walking the proto chain, with the
/// case-insensitive fallback Lingo callers need. `None` = absent.
fn resolve_js_object_prop(obj: &JsObjectRef, name: &str) -> Option<JsValue> {
    let mut current = Some(obj.clone());
    for _ in 0..32 {
        let o = current?;
        let b = o.borrow();
        if let Some(v) = b.get_own(name) {
            return Some(v.clone());
        }
        if let Some((_, v)) = b.props.iter().rev().find(|(k, _)| k.eq_ignore_ascii_case(name)) {
            return Some(v.clone());
        }
        current = b.proto.clone();
    }
    None
}

fn resolve_js_object_key(obj: &crate::player::js_lingo::value::JsObject, name: &str) -> String {
    obj.props
        .iter()
        .rev()
        .find(|(key, _)| key == name)
        .map(|(key, _)| key.clone())
        .or_else(|| {
            obj.props
                .iter()
                .rev()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(key, _)| key.clone())
        })
        .unwrap_or_else(|| name.to_owned())
}

fn owner_error(message: impl Into<String>) -> crate::player::ScriptError {
    crate::player::ScriptError::new_code(
        crate::player::ScriptErrorCode::InvalidReference,
        message.into(),
    )
}

fn js_error(error: JsError) -> crate::player::ScriptError {
    crate::player::ScriptError::new(error.message)
}

fn validate_datum_for_js(
    player: &crate::player::DirPlayer,
    symbols: &crate::player::symbols::symbol_table::SymbolTable,
    registry: &JsRuntimeRegistry,
    player_id: crate::player::session::PlayerId,
    owner: &crate::player::ownership::OwnerToken,
    datum_ref: &DatumRef,
) -> Result<(), crate::player::ScriptError> {
    crate::player::driver::validate_owned_datum_graph(player, symbols, datum_ref)?;
    let mut pending = vec![(true, datum_ref.clone(), 0usize)];
    let mut active = HashSet::new();
    while let Some((enter, reference, exit_id)) = pending.pop() {
        let DatumRef::Ref(_) = reference else {
            if !enter {
                active.remove(&exit_id);
            }
            continue;
        };
        let id = reference.unwrap();
        if !enter {
            active.remove(&exit_id);
            continue;
        }
        if !active.insert(id) {
            return Err(owner_error("cyclic Director datum cannot cross the JS boundary"));
        }
        let datum = player.allocator.try_get_datum(&reference).ok_or_else(|| {
            owner_error("foreign or stale JS conversion datum")
        })?;
        validate_direct_symbol_fields(datum, symbols)?;
        pending.push((false, DatumRef::Void, id));
        match datum {
            Datum::List(_, items, _) => {
                for item in items.iter().rev() {
                    pending.push((true, item.clone(), 0));
                }
            }
            Datum::PropList(pairs, _) => {
                for (key, value) in pairs.iter().rev() {
                    pending.push((true, value.clone(), 0));
                    pending.push((true, key.clone(), 0));
                }
            }
            Datum::StringChunk(
                crate::director::lingo::datum::StringChunkSource::Datum(child),
                _,
                _,
            ) => pending.push((true, child.clone(), 0)),
            Datum::TimeoutInstance(data) => {
                if let Some(script_instance) = &data.script_instance {
                    pending.push((true, script_instance.clone(), 0));
                }
                pending.push((true, data.target.clone(), 0));
                pending.push((true, data.callback.clone(), 0));
            }
            Datum::JsObjectRef(handle) => {
                if registry.object(player_id, owner, handle).is_none() {
                    return Err(owner_error("stale or foreign JS object"));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Invoke a method on a retained owner-bound object. Conversion borrows are
/// deliberately split around the interpreter call so bridge callbacks cannot
/// observe a borrowed VM context. The receiver remains `this` when lookup
/// finds the callable on a prototype.
pub(crate) fn invoke_js_object_method_explicit(
    session: &crate::player::session::RuntimeSessionHandle,
    player_id: crate::player::session::PlayerId,
    owner: &crate::player::ownership::OwnerToken,
    handle: &JsObjectHandle,
    name: &str,
    args: &[DatumRef],
) -> Result<DatumRef, crate::player::ScriptError> {
    let (runtime, object) = session
        .borrow()
        .js_lingo_registry()
        .object(player_id, owner, handle)
        .ok_or_else(|| owner_error("stale or foreign JS object"))?;
    let callee = resolve_js_object_prop(&object, name)
        .filter(|value| matches!(value, JsValue::Function(_) | JsValue::Native(_)))
        .ok_or_else(|| crate::player::ScriptError::new_code(
            crate::player::ScriptErrorCode::HandlerNotFound,
            format!("No handler {} for JS object datum", name),
        ))?;
    let runtime_for_conversion = runtime.clone();
    let js_args = session
        .borrow_mut()
        .with_player_js(player_id, |mut context, registry| {
            if !owner.is_arena_live() || !owner.same_identity(&context.player.owner) {
                return Err(owner_error("stale or foreign Director player"));
            }
            for arg in args {
                validate_datum_for_js(
                    context.player,
                    context.symbols,
                    registry,
                    player_id,
                    owner,
                    arg,
                )?;
            }
            let mut conversion = JsConversionContext {
                registry,
                runtime: &runtime_for_conversion,
                session: Rc::downgrade(session),
                player_id,
                owner: owner.clone(),
                active_arrays: HashSet::new(),
                active_objects: HashSet::new(),
            };
            args.iter()
                .map(|arg| datum_ref_to_js_value_with_context(context.player, context.symbols, &mut conversion, arg).map_err(js_error))
                .collect::<Result<Vec<_>, _>>()
        })
        .ok_or_else(|| owner_error("stale or foreign Director player"))??;
    let invocation = runtime.borrow().invoke(&callee, js_args, JsValue::Object(object));
    let runtime_for_conversion = runtime.clone();
    session
        .borrow_mut()
        .with_player_js(player_id, |mut context, registry| {
            if !owner.is_arena_live() || !owner.same_identity(&context.player.owner) {
                return Err(owner_error("stale or foreign Director player"));
            }
            if registry.object(player_id, owner, handle).is_none() {
                return Err(owner_error("stale or foreign JS object"));
            }
            let result = invocation.map_err(js_error)?;
            let mut conversion = JsConversionContext {
                registry,
                runtime: &runtime_for_conversion,
                session: Rc::downgrade(session),
                player_id,
                owner: owner.clone(),
                active_arrays: HashSet::new(),
                active_objects: HashSet::new(),
            };
            js_value_to_datum_ref_with_context(context.player, context.symbols, &mut conversion, &result).map_err(js_error)
        })
        .ok_or_else(|| owner_error("stale or foreign Director player"))?
}

pub(crate) fn get_js_object_prop_explicit(
    session: &crate::player::session::RuntimeSessionHandle,
    player_id: crate::player::session::PlayerId,
    owner: &crate::player::ownership::OwnerToken,
    handle: &JsObjectHandle,
    name: &str,
) -> Result<DatumRef, crate::player::ScriptError> {
    let (runtime, object) = session
        .borrow()
        .js_lingo_registry()
        .object(player_id, owner, handle)
        .ok_or_else(|| owner_error("stale or foreign JS object"))?;
    let Some(value) = resolve_js_object_prop(&object, name) else {
        return Ok(DatumRef::Void);
    };
    let runtime_for_conversion = runtime.clone();
    session
        .borrow_mut()
        .with_player_js(player_id, |mut context, registry| {
            if !owner.is_arena_live() || !owner.same_identity(&context.player.owner) {
                return Err(owner_error("stale or foreign Director player"));
            }
            if registry.object(player_id, owner, handle).is_none() {
                return Err(owner_error("stale or foreign JS object"));
            }
            let mut conversion = JsConversionContext {
                registry,
                runtime: &runtime_for_conversion,
                session: Rc::downgrade(session),
                player_id,
                owner: owner.clone(),
                active_arrays: HashSet::new(),
                active_objects: HashSet::new(),
            };
            js_value_to_datum_ref_with_context(context.player, context.symbols, &mut conversion, &value).map_err(js_error)
        })
        .ok_or_else(|| owner_error("stale or foreign Director player"))?
}

pub(crate) fn set_js_object_prop_explicit(
    session: &crate::player::session::RuntimeSessionHandle,
    player_id: crate::player::session::PlayerId,
    owner: &crate::player::ownership::OwnerToken,
    handle: &JsObjectHandle,
    name: &str,
    value_ref: &DatumRef,
) -> Result<(), crate::player::ScriptError> {
    let (runtime, object) = session
        .borrow()
        .js_lingo_registry()
        .object(player_id, owner, handle)
        .ok_or_else(|| owner_error("stale or foreign JS object"))?;
    let runtime_for_conversion = runtime.clone();
    let value = session
        .borrow_mut()
        .with_player_js(player_id, |mut context, registry| {
            if !owner.is_arena_live() || !owner.same_identity(&context.player.owner) {
                return Err(owner_error("stale or foreign Director player"));
            }
            validate_datum_for_js(context.player, context.symbols, registry, player_id, owner, value_ref)?;
            let mut conversion = JsConversionContext {
                registry,
                runtime: &runtime_for_conversion,
                session: Rc::downgrade(session),
                player_id,
                owner: owner.clone(),
                active_arrays: HashSet::new(),
                active_objects: HashSet::new(),
            };
            datum_ref_to_js_value_with_context(context.player, context.symbols, &mut conversion, value_ref).map_err(js_error)
        })
        .ok_or_else(|| owner_error("stale or foreign Director player"))??;
    session
        .borrow_mut()
        .with_player_js(player_id, |context, registry| {
            if !owner.is_arena_live()
                || !owner.same_identity(&context.player.owner)
                || registry.object(player_id, owner, handle).is_none()
            {
                return Err(owner_error("stale or foreign JS object"));
            }
            let mut object = object.borrow_mut();
            let key = resolve_js_object_key(&object, name);
            object.set_own(&key, value);
            Ok(())
        })
        .ok_or_else(|| owner_error("stale or foreign Director player"))?
}

/// Public entry called from cast_lib::insert_member at script-load time.
/// Detects whether a Script is JS-Lingo, sets up the runtime, and emits
/// a disassembly to the log for diagnostics.
pub fn diagnose_js_script(script: &Script) -> Option<JsScriptRegistration> {
    let Some(payload) = extract_js_payload(script) else { return None; };
    log::info!(
        "[js-lingo] {}:{} loading JSScript ({} bytes)",
        script.member_ref.cast_lib, script.member_ref.cast_member, payload.len()
    );
    match decode_script(payload) {
        Ok(ir) => {
            for line in disassemble(&ir).lines() {
                log::info!("[js-lingo]   {}", line);
            }
            return Some(JsScriptRegistration {
                member_ref: script.member_ref.clone(),
                ir,
            });
        }
        Err(e) => {
            log::warn!("[js-lingo]   decode failed: {}", e);
        }
    }
    None
}

/// Owned result of decoding a JS-Lingo cast member. Cast parsing may happen
/// under a cast-library borrow; installation is performed later by the
/// session-owned loader boundary after that borrow ends.
pub struct JsScriptRegistration {
    pub member_ref: CastMemberRef,
    pub ir: JsScriptIR,
}

/// Install a parsed script against its owning session and execute its
/// top-level program before publishing the runtime for handler lookup.
pub fn register_js_script(
    session: crate::player::session::RuntimeSessionHandle,
    player_id: crate::player::session::PlayerId,
    owner: crate::player::ownership::OwnerToken,
    registration: JsScriptRegistration,
) -> Result<(), JsError> {
    let valid_before = session.borrow_mut().with_player(player_id, |context| {
        context.player.owner.is_arena_live() && owner.same_identity(&context.player.owner)
    }).unwrap_or(false);
    if !valid_before {
        return Err(JsError::new("stale or foreign Director player"));
    }
    let runtime = Rc::new(RefCell::new(JsRuntime::with_stdlib()));
    let bridge: Rc<RefCell<dyn JsHostBridge>> = Rc::new(RefCell::new(PlayerBridge {
        session: Rc::downgrade(&session),
        player_id,
        owner: owner.clone(),
        runtime: Rc::downgrade(&runtime),
    }));
    runtime.borrow_mut().set_bridge(bridge);
    runtime.borrow().install_director_globals();
    let ir = Rc::new(registration.ir);
    runtime.borrow_mut().run_program(&ir)?;
    let valid_after = session.borrow_mut().with_player(player_id, |context| {
        context.player.owner.is_arena_live() && owner.same_identity(&context.player.owner)
    }).unwrap_or(false);
    if !valid_after {
        return Err(JsError::new("Director player was reset during JS initialization"));
    }
    let member_ref = registration.member_ref;
    session.borrow_mut().js_lingo_registry_mut().insert_runtime(
        player_id,
        owner,
        member_ref.clone(),
        runtime.clone(),
    );
    Ok(())
}

/// Look up the JSScript payload from the literals area, if present.
fn extract_js_payload(script: &Script) -> Option<&[u8]> {
    for lit in &script.chunk.literals {
        if let Datum::JavaScript(b) = lit {
            return Some(b);
        }
    }
    None
}

/// Owner-bound handler entry used by the session executor. The runtime and
/// all Datum conversions are resolved through the supplied session; no
/// ambient player or process-global registry is consulted. The session is
/// borrowed only before and after the interpreter invocation.
pub fn try_invoke_js_handler_explicit(
    session: &crate::player::session::RuntimeSessionHandle,
    player_id: crate::player::session::PlayerId,
    owner: &crate::player::ownership::OwnerToken,
    script_member_ref: &CastMemberRef,
    handler_name: &str,
    args: &[DatumRef],
    _has_receiver: bool,
) -> Option<Result<DatumRef, String>> {
    let runtime = {
        let session_ref = session.borrow();
        session_ref.js_lingo_registry().runtime(player_id, owner, script_member_ref)
    }?;
    let callee = {
        let runtime_ref = runtime.borrow();
        let global = runtime_ref.global.borrow();
        global.get_own(handler_name).cloned()
    }?;
    if !matches!(callee, JsValue::Function(_) | JsValue::Native(_)) {
        return None;
    }
    let runtime_for_conversion = runtime.clone();
    let mut js_args = match session.borrow_mut().with_player_js(player_id, |mut context, registry| {
        if !context.player.owner.is_arena_live() || !owner.same_identity(&context.player.owner) {
            return Err(JsError::new("stale or foreign Director player"));
        }
        for datum in args {
            validate_datum_for_js(
                context.player,
                context.symbols,
                registry,
                player_id,
                owner,
                datum,
            ).map_err(|error| JsError::new(error.message))?;
        }
        let mut conversion = JsConversionContext {
            registry,
            runtime: &runtime_for_conversion,
            session: Rc::downgrade(session),
            player_id,
            owner: owner.clone(),
            active_arrays: HashSet::new(),
            active_objects: HashSet::new(),
        };
        args.iter()
            .map(|datum| datum_ref_to_js_value_with_context(context.player, context.symbols, &mut conversion, datum))
            .collect::<Result<Vec<_>, _>>()
    }) {
        Some(Ok(values)) => values,
        Some(Err(error)) => return Some(Err(error.message)),
        None => return Some(Err("stale or foreign Director player".to_owned())),
    };
    if let JsValue::Function(function) = &callee {
        let first_arg_is_me = function.atom.bindings.iter()
            .find(|binding| binding.kind == super::js_lingo::xdr::JsBindingKind::Argument)
            .map(|binding| matches!(binding.name.to_lowercase().as_str(), "me" | "mee" | "self" | "_me" | "this_" | "_self"))
            .unwrap_or(false);
        if first_arg_is_me {
            js_args.insert(0, JsValue::Undefined);
        }
    }
    // Keep the interpreted `this` value owner-bound to the script proxy. A
    // global object here loses the same-runtime receiver identity and makes
    // script-to-script reentry observe the wrong properties.
    let this_value = script_ref_to_js_proxy_with_session(
        script_member_ref.clone(),
        Rc::downgrade(session),
        player_id,
        owner.clone(),
    );
    let invocation = runtime.borrow().invoke(&callee, js_args, this_value);
    let runtime_for_conversion = runtime.clone();
    Some(match session.borrow_mut().with_player_js(player_id, |mut context, registry| {
        if !context.player.owner.is_arena_live() || !owner.same_identity(&context.player.owner) {
            return Err(JsError::new("stale or foreign Director player"));
        }
        if registry.runtime(player_id, owner, script_member_ref).is_none() {
            return Err(JsError::new("stale or foreign script runtime"));
        }
        let result = invocation.as_ref().map_err(|error| JsError::new(error.message.clone()))?;
        let mut conversion = JsConversionContext {
            registry,
            runtime: &runtime_for_conversion,
            session: Rc::downgrade(session),
            player_id,
            owner: owner.clone(),
            active_arrays: HashSet::new(),
            active_objects: HashSet::new(),
        };
        js_value_to_datum_ref_with_context(context.player, context.symbols, &mut conversion, result)
    }) {
        Some(Ok(value)) => Ok(value),
        Some(Err(error)) => Err(error.message),
        None => Err("stale or foreign Director player".to_owned()),
    })
}

/// Test whether an owner-bound JS runtime contains a callable handler without
/// invoking it. Setup uses this classification before allocating a VM scope;
/// the actual call is performed by the external setup executor.
pub(crate) fn has_js_handler_explicit(
    session: &mut crate::player::session::RuntimeSession,
    player_id: crate::player::session::PlayerId,
    owner: &crate::player::ownership::OwnerToken,
    script_member_ref: &CastMemberRef,
    handler_name: &str,
) -> bool {
    let Some(runtime) = session
        .js_lingo_registry()
        .runtime(player_id, owner, script_member_ref)
    else {
        return false;
    };
    let runtime_ref = runtime.borrow();
    let global = runtime_ref.global.borrow();
    matches!(
        global.get_own(handler_name),
        Some(JsValue::Function(_) | JsValue::Native(_))
    )
}

/// Execute one setup callback after the VM borrow has ended. The request owns
/// all argument values and the exact owner token; this function only takes
/// short session borrows to materialize arguments and to validate the returned
/// datum. A missing callable is reported as a typed completion error so the
/// driver cannot silently allocate a fallback scope or fabricate a value.
pub(crate) fn execute_setup_callback(
    session: crate::player::session::RuntimeSessionHandle,
    request: crate::player::driver::SetupCallbackRequest,
) -> crate::player::driver::ActionCompletion {
    let player_id = request.player_id;
    let owner = request.owner.clone();
    let parent_scope = request.expectation.parent_scope();
    if !session.borrow().validate_setup_action(
        &request.ticket,
        &owner,
        parent_scope.as_ref(),
    ) {
        return crate::player::driver::ActionCompletion::InternalError(
            crate::player::ScriptError::new_code(
                crate::player::ScriptErrorCode::InvalidReference,
                "stale setup callback ticket".to_owned(),
            ),
        );
    }
    let args = match session.borrow_mut().with_player(player_id, |context| {
        if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
            return Err(crate::player::ScriptError::new_code(
                crate::player::ScriptErrorCode::InvalidReference,
                "stale setup callback owner".to_owned(),
            ));
        }
        if !request.expectation.validate(context.player) {
            return Err(crate::player::ScriptError::new_code(
                crate::player::ScriptErrorCode::InvalidReference,
                "stale setup callback scope".to_owned(),
            ));
        }
        Ok(request
            .args
            .iter()
            .cloned()
            .map(|datum| context.player.alloc_datum(datum))
            .collect::<Vec<_>>())
    }) {
        Some(Ok(args)) => args,
        Some(Err(error)) => return crate::player::driver::ActionCompletion::InternalError(error),
        None => {
            return crate::player::driver::ActionCompletion::InternalError(
                crate::player::ScriptError::new_code(
                    crate::player::ScriptErrorCode::InvalidReference,
                    "setup callback player was removed".to_owned(),
                ),
            )
        }
    };
    let Some(result) = try_invoke_js_handler_explicit(
        &session,
        player_id,
        &owner,
        &request.script_ref,
        &request.handler_name,
        &args,
        request.receiver.is_some(),
    ) else {
        return crate::player::driver::ActionCompletion::InternalError(
            crate::player::ScriptError::new_code(
                crate::player::ScriptErrorCode::HandlerNotFound,
                format!("JS setup handler {} was removed", request.handler_name),
            ),
        );
    };
    match result {
        Ok(value) => crate::player::driver::ActionCompletion::InternalResult(value),
        Err(message) => crate::player::driver::ActionCompletion::InternalError(
            crate::player::ScriptError::new(format!(
                "JS handler {} threw: {}",
                request.handler_name, message
            )),
        ),
    }
}

// ===== Bridge implementation that routes JS calls through Director runtime =====

struct PlayerBridge {
    session: std::rc::Weak<std::cell::RefCell<crate::player::session::RuntimeSession>>,
    player_id: crate::player::session::PlayerId,
    owner: crate::player::ownership::OwnerToken,
    runtime: Weak<RefCell<JsRuntime>>,
}

impl PlayerBridge {
    fn with_context<R>(&self, f: impl FnOnce(&mut crate::player::session::ExecutionContext<'_>, &mut JsRuntimeRegistry) -> R) -> Option<R> {
        let session = self.session.upgrade()?;
        let mut session = session.borrow_mut();
        let owner = self.owner.clone();
        session.with_player_js(self.player_id, |mut context, registry| {
            if !context.player.owner.is_arena_live() || !owner.same_identity(&context.player.owner) {
                return None;
            }
            Some(f(&mut context, registry))
        })?
    }

    fn args(&self, context: &mut crate::player::session::ExecutionContext<'_>, registry: &mut JsRuntimeRegistry, args: &[JsValue]) -> Result<Vec<DatumRef>, JsError> {
        let runtime = self.runtime.upgrade().ok_or_else(|| JsError::new("Director JS runtime disposed"))?;
        let mut conversion = JsConversionContext {
            registry,
            runtime: &runtime,
            session: self.session.clone(),
            player_id: self.player_id,
            owner: self.owner.clone(),
            active_arrays: HashSet::new(),
            active_objects: HashSet::new(),
        };
        args.iter().map(|value| js_value_to_datum_ref_with_context(context.player, context.symbols, &mut conversion, value)).collect()
    }

    fn value(&self, context: &mut crate::player::session::ExecutionContext<'_>, registry: &mut JsRuntimeRegistry, datum: &DatumRef) -> Result<JsValue, JsError> {
        let runtime = self.runtime.upgrade().ok_or_else(|| JsError::new("Director JS runtime disposed"))?;
        let mut conversion = JsConversionContext {
            registry,
            runtime: &runtime,
            session: self.session.clone(),
            player_id: self.player_id,
            owner: self.owner.clone(),
            active_arrays: HashSet::new(),
            active_objects: HashSet::new(),
        };
        datum_ref_to_js_value_with_context(context.player, context.symbols, &mut conversion, datum)
    }
}

impl JsHostBridge for PlayerBridge {
    fn trace(&mut self, args: &[JsValue]) {
        let line = args.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(" ");
        log::info!("[js-trace] {}", line);
    }
    fn sprite(&mut self, channel: i32) -> JsValue {
        // Phase 6.4: live sprite proxy. Property reads / writes routed via
        // get_property / set_property in the interpreter round-trip through
        // sprite_get_prop / sprite_set_prop, so `sprite(3).locH = 100`
        // actually moves the sprite on stage.
        JsValue::DirectorRef(crate::player::js_lingo::value::DirectorRefKind::Sprite(channel as i16))
    }
    fn member(&mut self, args: &[JsValue]) -> JsValue {
        // member(memberNameOrNum {, castNameOrNum}). The Lingo VM's
        // existing "member" handler accepts the same shape and resolves
        // the name to a CastMemberRef -- route through it so we get
        // consistent name/number/cast-lib resolution, then wrap the
        // resulting Datum back as a DirectorRef.
        match self.with_context(|context, registry| -> Result<JsValue, JsError> {
            let datum_args = self.args(context, registry, args)?;
            let symbol = context.symbols.intern("member");
            match crate::player::handlers::manager::BuiltInHandlerManager::call_handler(
                context, symbol, &datum_args,
            ) {
                Ok(dref) => Ok(self.value(context, registry, &dref)?),
                Err(_) => Ok(JsValue::Undefined),
            }
        }) {
            Some(Ok(value)) => value,
            Some(Err(_)) | None => JsValue::Undefined,
        }
    }

    /// Fall-through for anything else the JS script tries to invoke as a
    /// global. We route through `BuiltInHandlerManager::call_handler`, which
    /// is the same registry Lingo uses for `gotoNetPage`, `getNetText`,
    /// `puppetTempo`, `count`, `script`, etc.
    fn call_global(&mut self, name: &str, args: &[JsValue]) -> Result<JsValue, JsError> {
        let result = self.with_context(|context, registry| -> Result<JsValue, JsError> {
            let datum_args = self.args(context, registry, args)?;
            let symbol = context.symbols.intern(name);
            match crate::player::handlers::manager::BuiltInHandlerManager::call_handler(
                context, symbol, &datum_args,
            ) {
                Ok(dref) => Ok(self.value(context, registry, &dref)?),
                Err(e) => Err(JsError::new(e.message)),
            }
        }).ok_or_else(|| JsError::new("stale or foreign Director player"))?
            .map_err(|e| JsError::new(format!("{}: {}", name, e.message)))?;
        Ok(result)
    }

    fn director_ref_get_property(&mut self, kind: &crate::player::js_lingo::value::DirectorRefKind, name: &str) -> JsValue {
        self.with_context(|context, registry| {
            let property = context.symbols.intern(name);
            let result = match kind {
                crate::player::js_lingo::value::DirectorRefKind::Sprite(channel) =>
                    crate::player::score::sprite_get_prop(context.player, context.symbols, *channel, property),
                crate::player::js_lingo::value::DirectorRefKind::Member { cast_lib, cast_member } => {
                    let member = CastMemberRef { cast_lib: *cast_lib, cast_member: *cast_member };
                    use crate::player::handlers::datum_handlers::cast_member_ref::CastMemberRefHandlers;
                    CastMemberRefHandlers::get_prop(context.player, context.symbols, &member, property)
                }
            };
            result.ok().map(|datum| {
                let reference = context.player.alloc_datum(datum);
                self.value(context, registry, &reference).unwrap_or(JsValue::Undefined)
            }).unwrap_or(JsValue::Undefined)
        }).unwrap_or(JsValue::Undefined)
    }

    fn director_ref_set_property(&mut self, kind: &crate::player::js_lingo::value::DirectorRefKind, name: &str, value: JsValue) -> Result<(), JsError> {
        self.with_context(|context, registry| {
            let property = context.symbols.intern(name);
            let runtime = self.runtime.upgrade().ok_or_else(|| JsError::new("Director JS runtime disposed"))?;
            let mut conversion = JsConversionContext {
                registry,
                runtime: &runtime,
                session: self.session.clone(),
                player_id: self.player_id,
            owner: self.owner.clone(),
            active_arrays: HashSet::new(),
            active_objects: HashSet::new(),
        };
            let reference = js_value_to_datum_ref_with_context(context.player, context.symbols, &mut conversion, &value)?;
            let datum = context.player.get_datum(&reference).clone();
            let result = match kind {
                crate::player::js_lingo::value::DirectorRefKind::Sprite(channel) =>
                    crate::player::score::sprite_set_prop(context.player, context.symbols, *channel, property, datum),
                crate::player::js_lingo::value::DirectorRefKind::Member { cast_lib, cast_member } => {
                    let member = CastMemberRef { cast_lib: *cast_lib, cast_member: *cast_member };
                    use crate::player::handlers::datum_handlers::cast_member_ref::CastMemberRefHandlers;
                    CastMemberRefHandlers::set_prop(context.player, context.symbols, &member, property, datum)
                }
            };
            result.map_err(|error| JsError::new(error.message))
        }).ok_or_else(|| JsError::new("stale or foreign Director player"))?
    }
}

// ===== Datum <-> JsValue =====

/// Convert a DatumRef (Lingo value) into a JsValue for use in JS code.
///
/// Coverage: primitives convert directly. Lists become Arrays, prop-lists
/// become Objects (lossy because Lingo prop-lists are case-insensitive).
/// References (Sprite, Member, etc.) wrap into JsObjects tagged with
/// the original Datum string form so reads still see something useful.
fn datum_ref_to_js_value_plain(
    player: &mut crate::player::DirPlayer,
    symbols: &mut crate::player::symbols::symbol_table::SymbolTable,
    dref: &DatumRef,
) -> Result<JsValue, JsError> {
    let d = player.allocator.get_datum(dref).clone();
    match d {
        Datum::Int(i) => Ok(JsValue::Int(i)),
        Datum::Float(f) => Ok(JsValue::Number(f)),
        Datum::String(s) => Ok(JsValue::String(Rc::new(s))),
        Datum::Symbol(s) => Ok(JsValue::String(Rc::new(symbols.display(&s)
            .map_err(|_| JsError::new("foreign or stale symbol"))?.to_owned()))),
        Datum::Void | Datum::Null => Ok(JsValue::Undefined),
        Datum::List(_, items, _) => {
            let arr: Vec<JsValue> = items.iter().map(|r| datum_ref_to_js_value_plain(player, symbols, r)).collect::<Result<_, _>>()?;
            Ok(JsValue::Array(Rc::new(RefCell::new(crate::player::js_lingo::value::JsArray { items: arr }))))
        }
        Datum::PropList(pairs, _) => {
            let mut obj = crate::player::js_lingo::value::JsObject::new();
            for (k_ref, v_ref) in pairs {
                let key = datum_ref_to_string(player, symbols, &k_ref)?;
                let v = datum_ref_to_js_value_plain(player, symbols, &v_ref)?;
                obj.set_own(&key, v);
            }
            Ok(JsValue::Object(Rc::new(RefCell::new(obj))))
        }
        Datum::ScriptRef(member_ref) => Ok(script_ref_to_js_proxy(member_ref)),
        // Round-trip a live handle back to the very same JS object, so
        // identity survives a Lingo→JS→Lingo hop
        Datum::JsObjectRef(_) => Err(JsError::new("JS object conversion requires an owner-bound runtime")),
        // Live Director-owned references. Property reads/writes round-trip
        // through datum_handlers via get_property / set_property in the
        // interpreter, so `sprite(3).locH = 100` actually moves the sprite
        // and reads back the new value.
        Datum::SpriteRef(n) => Ok(JsValue::DirectorRef(
            crate::player::js_lingo::value::DirectorRefKind::Sprite(n),
        )),
        Datum::CastMember(mref) => Ok(JsValue::DirectorRef(
            crate::player::js_lingo::value::DirectorRefKind::Member {
                cast_lib: mref.cast_lib,
                cast_member: mref.cast_member,
            },
        )),
        _ => {
            // Coarse fallback: stringify via the existing formatter so refs
            // like `(member 3 of castLib 1)` keep their readable form.
            let s = crate::player::datum_formatting::format_datum(dref, symbols, player)
                .map_err(|error| JsError::new(error.message))?;
            Ok(JsValue::String(Rc::new(s)))
        }
    }
}

/// Wrap a `Datum::ScriptRef` as a Director proxy. Handler registration is
/// performed by the owning runtime boundary; this conversion only carries
/// the stable cast coordinates and never consults process-global state.
fn script_ref_to_js_proxy(member_ref: CastMemberRef) -> JsValue {
    use crate::player::js_lingo::value::JsObject;
    let mut obj = JsObject::new();
    obj.class_name = "ScriptRef";
    // Mirror the script-ref coordinates so the proxy round-trips back to a
    // DatumRef when JS hands it to another Lingo builtin.
    obj.set_own("__script_lib__", JsValue::Int(member_ref.cast_lib));
    obj.set_own("__script_member__", JsValue::Int(member_ref.cast_member));

    JsValue::Object(Rc::new(RefCell::new(obj)))
}

/// Build a script proxy for an executing, owner-bound runtime. Native method
/// closures capture only a weak session handle and identity metadata; each
/// invocation upgrades and borrows the session in short preparation and
/// application phases, so no VM borrow survives the interpreter call.
fn script_ref_to_js_proxy_with_session(
    member_ref: CastMemberRef,
    session: Weak<RefCell<crate::player::session::RuntimeSession>>,
    player_id: crate::player::session::PlayerId,
    owner: crate::player::ownership::OwnerToken,
) -> JsValue {
    let runtime = session.upgrade().and_then(|handle| {
        let session_ref = handle.borrow();
        session_ref.js_lingo_registry().runtime(player_id, &owner, &member_ref)
    });
    let Some(runtime) = runtime else {
        return script_ref_to_js_proxy(member_ref);
    };
    script_ref_to_js_proxy_with_runtime(member_ref, session, player_id, owner, runtime)
}

/// Build a script proxy from an already-resolved runtime snapshot. This helper
/// never borrows the RuntimeSession; it is safe to call while a short
/// `with_player_js` conversion borrow is active.
fn script_ref_to_js_proxy_with_runtime(
    member_ref: CastMemberRef,
    session: Weak<RefCell<crate::player::session::RuntimeSession>>,
    player_id: crate::player::session::PlayerId,
    owner: crate::player::ownership::OwnerToken,
    runtime: Rc<RefCell<JsRuntime>>,
) -> JsValue {
    use crate::player::js_lingo::value::{JsObject, NativeFn};
    let mut obj = JsObject::new();
    obj.class_name = "ScriptRef";
    obj.set_own("__script_lib__", JsValue::Int(member_ref.cast_lib));
    obj.set_own("__script_member__", JsValue::Int(member_ref.cast_member));
    let handler_names: Vec<String> = {
        let rt = runtime.borrow();
        let global = rt.global.borrow();
        global.props.iter().filter_map(|(key, value)| match value {
            JsValue::Function(_) | JsValue::Native(_) => Some(key.clone()),
            _ => None,
        }).collect()
    };
    for name in handler_names {
        let session_ref = session.clone();
        let target = member_ref.clone();
        let target_name = name.clone();
        let target_owner = owner.clone();
        let native = NativeFn {
            name: "<script_method>",
            call: Box::new(move |args| invoke_script_method_explicit(
                &session_ref, player_id, &target_owner, &target, &target_name, args,
            )),
        };
        obj.set_own(&name, JsValue::Native(Rc::new(native)));
    }
    JsValue::Object(Rc::new(RefCell::new(obj)))
}

fn invoke_script_method_explicit(
    session: &Weak<RefCell<crate::player::session::RuntimeSession>>,
    player_id: crate::player::session::PlayerId,
    owner: &crate::player::ownership::OwnerToken,
    member_ref: &CastMemberRef,
    handler_name: &str,
    args: &[JsValue],
) -> Result<JsValue, JsError> {
    let session = session.upgrade().ok_or_else(|| JsError::new("Director session disposed"))?;
    let runtime = {
        let session_ref = session.borrow();
        session_ref.js_lingo_registry().runtime(player_id, owner, member_ref)
            .ok_or_else(|| JsError::new("stale or foreign script runtime"))?
    };
    let callee = {
        let runtime_ref = runtime.borrow();
        let global = runtime_ref.global.borrow();
        global.get_own(handler_name).cloned()
            .ok_or_else(|| JsError::new(format!("script handler {handler_name} not found")))?
    };
    if !matches!(callee, JsValue::Function(_) | JsValue::Native(_)) {
        return Err(JsError::new(format!("script handler {handler_name} is not callable")));
    }
    let js_args = {
        let mut session_ref = session.borrow_mut();
        let runtime_for_conversion = runtime.clone();
        session_ref.with_player_js(player_id, |mut context, registry| {
            if !context.player.owner.is_arena_live() || !owner.same_identity(&context.player.owner) {
                return Err(JsError::new("stale or foreign Director player"));
            }
            let mut conversion = JsConversionContext {
                registry,
                runtime: &runtime_for_conversion,
                session: Rc::downgrade(&session),
                player_id,
                owner: owner.clone(),
                active_arrays: HashSet::new(),
                active_objects: HashSet::new(),
            };
            args.iter()
                .map(|value| js_value_to_datum_ref_with_context(context.player, context.symbols, &mut conversion, value))
                .collect::<Result<Vec<_>, _>>()
        }).ok_or_else(|| JsError::new("stale or foreign Director player"))??
    };
    let js_args = {
        let mut converted = Vec::with_capacity(js_args.len());
        let mut session_ref = session.borrow_mut();
        let runtime_for_conversion = runtime.clone();
        session_ref.with_player_js(player_id, |mut context, registry| {
            if !context.player.owner.is_arena_live() || !owner.same_identity(&context.player.owner) {
                return Err(JsError::new("stale or foreign Director player"));
            }
            let mut conversion = JsConversionContext {
                registry,
                runtime: &runtime_for_conversion,
                session: Rc::downgrade(&session),
                player_id,
                owner: owner.clone(),
                active_arrays: HashSet::new(),
                active_objects: HashSet::new(),
            };
            for datum in js_args {
                converted.push(datum_ref_to_js_value_with_context(context.player, context.symbols, &mut conversion, &datum)?);
            }
            Ok::<_, JsError>(converted)
        }).ok_or_else(|| JsError::new("stale or foreign Director player"))??
    };
    let this_value = script_ref_to_js_proxy_with_session(
        member_ref.clone(),
        Rc::downgrade(&session),
        player_id,
        owner.clone(),
    );
    let invocation = runtime.borrow().invoke(&callee, js_args, this_value);
    let datum = {
        let mut session_ref = session.borrow_mut();
        let runtime_for_conversion = runtime.clone();
        session_ref.with_player_js(player_id, |mut context, registry| {
            if !context.player.owner.is_arena_live() || !owner.same_identity(&context.player.owner) {
                return Err(JsError::new("stale or foreign Director player"));
            }
            if registry.runtime(player_id, owner, member_ref).is_none() {
                return Err(JsError::new("stale or foreign script runtime"));
            }
            let result = invocation
                .as_ref()
                .map_err(|error| JsError::new(error.message.clone()))?;
            let mut conversion = JsConversionContext {
                registry,
                runtime: &runtime_for_conversion,
                session: Rc::downgrade(&session),
                player_id,
                owner: owner.clone(),
                active_arrays: HashSet::new(),
                active_objects: HashSet::new(),
            };
            js_value_to_datum_ref_with_context(context.player, context.symbols, &mut conversion, result)
        }).ok_or_else(|| JsError::new("stale or foreign Director player"))??
    };
    let mut session_ref = session.borrow_mut();
    let runtime_for_conversion = runtime.clone();
    session_ref.with_player_js(player_id, |mut context, registry| {
        if !context.player.owner.is_arena_live() || !owner.same_identity(&context.player.owner) {
            return Err(JsError::new("stale or foreign Director player"));
        }
        let mut conversion = JsConversionContext {
            registry,
            runtime: &runtime_for_conversion,
            session: Rc::downgrade(&session),
            player_id,
            owner: owner.clone(),
            active_arrays: HashSet::new(),
            active_objects: HashSet::new(),
        };
        datum_ref_to_js_value_with_context(context.player, context.symbols, &mut conversion, &datum)
    }).ok_or_else(|| JsError::new("stale or foreign Director player"))?
}

fn datum_ref_to_string(
    player: &mut crate::player::DirPlayer,
    symbols: &crate::player::symbols::symbol_table::SymbolTable,
    dref: &DatumRef,
) -> Result<String, JsError> {
    let d = player.allocator.get_datum(dref).clone();
    match d {
        Datum::String(s) => Ok(s),
        Datum::Symbol(s) => Ok(symbols.display(&s).map_err(|_| JsError::new("foreign or stale symbol"))?.to_owned()),
        Datum::Int(i) => Ok(i.to_string()),
        Datum::Float(f) => Ok(f.to_string()),
        _ => crate::player::datum_formatting::format_datum(dref, symbols, player)
            .map_err(|error| JsError::new(error.message)),
    }
}

/// Convert a JsValue back into a DatumRef so the Lingo VM can consume it.
fn js_value_to_datum_ref_plain(
    player: &mut crate::player::DirPlayer,
    symbols: &mut crate::player::symbols::symbol_table::SymbolTable,
    v: &JsValue,
) -> Result<DatumRef, JsError> {
    let datum = match v {
        JsValue::Undefined => Datum::Void,
        JsValue::Null => Datum::Null,
        JsValue::Bool(b) => Datum::Int(if *b { 1 } else { 0 }),
        JsValue::Int(i) => Datum::Int(*i),
        JsValue::Number(n) => Datum::Float(*n),
        JsValue::String(s) => Datum::String((**s).clone()),
        JsValue::Array(a) => {
            // Convert each element to a DatumRef and wrap in a Lingo list.
            let items: std::collections::VecDeque<DatumRef> = a
                .borrow()
                .items
                .iter()
                .map(|x| js_value_to_datum_ref_plain(player, symbols, x))
                .collect::<Result<_, _>>()?;
            Datum::List(crate::director::lingo::datum::DatumType::List, items, false)
        }
        JsValue::Object(o) if object_has_callable(o) => {
            return Err(JsError::new("callable JS object requires an owner-bound runtime"));
        }
        JsValue::Object(o) => {
            // Plain data objects retain the historical PropList conversion.
            // Callable objects are rejected above because an owner-bound
            // registry is required to preserve their identity and methods.
            let pairs: std::collections::VecDeque<crate::director::lingo::datum::PropListPair> = o
                .borrow()
                .props
                .iter()
                .map(|(k, val)| -> Result<crate::director::lingo::datum::PropListPair, JsError> {
                    let key_dr = player.alloc_datum(Datum::Symbol(symbols.intern(k)));
                    let val_dr = js_value_to_datum_ref_plain(player, symbols, val)?;
                    Ok((key_dr, val_dr))
                })
                .collect::<Result<_, _>>()?;
            Datum::PropList(pairs, false)
        }
        JsValue::Function(_) | JsValue::Native(_) => {
            return Err(JsError::new("callable JS value requires an owner-bound runtime"));
        }
        JsValue::Iterator(_) => Datum::String("[for-in iter]".to_string()),
        JsValue::DirectorRef(k) => {
            use crate::player::js_lingo::value::DirectorRefKind;
            match k {
                DirectorRefKind::Sprite(n) => Datum::SpriteRef(*n),
                DirectorRefKind::Member { cast_lib, cast_member } => {
                    Datum::CastMember(crate::player::cast_lib::CastMemberRef {
                        cast_lib: *cast_lib,
                        cast_member: *cast_member,
                    })
                }
            }
        }
    };
    Ok(player.alloc_datum(datum))
}

/// Owner-bound counterpart to `datum_ref_to_js_value`. Every live object and
/// script proxy is attributed to the supplied session registry and runtime.
fn datum_ref_to_js_value_with_context(
    player: &mut crate::player::DirPlayer,
    symbols: &mut crate::player::symbols::symbol_table::SymbolTable,
    context: &mut JsConversionContext<'_>,
    dref: &DatumRef,
) -> Result<JsValue, JsError> {
    let datum = player.allocator.get_datum(dref).clone();
    match datum {
        Datum::List(_, items, _) => {
            let values = items.iter()
                .map(|item| datum_ref_to_js_value_with_context(player, symbols, context, item))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(JsValue::Array(Rc::new(RefCell::new(crate::player::js_lingo::value::JsArray { items: values }))))
        }
        Datum::PropList(pairs, _) => {
            let mut object = crate::player::js_lingo::value::JsObject::new();
            for (key, value) in pairs {
                let key = datum_ref_to_string(player, symbols, &key)?;
                let value = datum_ref_to_js_value_with_context(player, symbols, context, &value)?;
                object.set_own(&key, value);
            }
            Ok(JsValue::Object(Rc::new(RefCell::new(object))))
        }
        Datum::ScriptRef(member_ref) => {
            let runtime = context
                .registry
                .runtime(context.player_id, &context.owner, &member_ref)
                .ok_or_else(|| JsError::new("stale or foreign script runtime"))?;
            Ok(script_ref_to_js_proxy_with_runtime(
                member_ref,
                context.session.clone(),
                context.player_id,
                context.owner.clone(),
                runtime,
            ))
        }
        Datum::JsObjectRef(handle) => {
            let (_, object) = context.registry.object(context.player_id, &context.owner, &handle)
                .ok_or_else(|| JsError::new("stale or foreign JS object"))?;
            Ok(JsValue::Object(object))
        }
        _ => datum_ref_to_js_value_plain(player, symbols, dref),
    }
}

/// Owner-bound JS-to-Datum conversion. Callable objects are assigned a
/// session-scoped handle before the interpreter borrow ends; nested arrays and
/// records use this same context recursively.
fn js_value_to_datum_ref_with_context(
    player: &mut crate::player::DirPlayer,
    symbols: &mut crate::player::symbols::symbol_table::SymbolTable,
    context: &mut JsConversionContext<'_>,
    value: &JsValue,
) -> Result<DatumRef, JsError> {
    match value {
        JsValue::Array(array) => {
            let identity = Rc::as_ptr(array) as usize;
            if !context.active_arrays.insert(identity) {
                return Err(JsError::new("cyclic JS array cannot cross the Director boundary"));
            }
            let items = array.borrow().items.clone().into_iter()
                .map(|item| js_value_to_datum_ref_with_context(player, symbols, context, &item))
                .collect::<Result<std::collections::VecDeque<_>, _>>();
            context.active_arrays.remove(&identity);
            let items = items?;
            Ok(player.alloc_datum(Datum::List(crate::director::lingo::datum::DatumType::List, items, false)))
        }
        JsValue::Object(object) if object_has_callable(object) => {
            let handle = context.registry.register_object(
                context.player_id,
                &context.owner,
                context.runtime,
                object,
            )?;
            Ok(player.alloc_datum(Datum::JsObjectRef(handle)))
        }
        JsValue::Object(object) => {
            let identity = Rc::as_ptr(object) as usize;
            if !context.active_objects.insert(identity) {
                return Err(JsError::new("cyclic JS object cannot cross the Director boundary"));
            }
            let pairs = object.borrow().props.clone().into_iter()
                .map(|(key, value)| {
                    let key_ref = player.alloc_datum(Datum::Symbol(symbols.intern(&key)));
                    let value_ref = js_value_to_datum_ref_with_context(player, symbols, context, &value)?;
                    Ok((key_ref, value_ref))
                })
                .collect::<Result<std::collections::VecDeque<_>, JsError>>();
            context.active_objects.remove(&identity);
            let pairs = pairs?;
            Ok(player.alloc_datum(Datum::PropList(pairs, false)))
        }
        _ => js_value_to_datum_ref_plain(player, symbols, value),
    }
}

#[cfg(test)]
mod registry_tests {
    use super::*;
    use async_std::channel;
    use crate::director::lingo::datum::Datum;
    use crate::player::ownership::{OwnerKey, OwnerToken};
    use crate::player::session::RuntimeSession;
    use crate::player::symbols::symbol_table::SymbolOwner;

    fn owner(player: u64, generation: u64) -> OwnerToken {
        OwnerToken::new(OwnerKey {
            session: 91,
            player,
            generation,
        })
    }

    fn object() -> JsObjectRef {
        Rc::new(RefCell::new(crate::player::js_lingo::value::JsObject::new()))
    }

    fn session_with_object(
        session_id: u64,
    ) -> (crate::player::session::RuntimeSessionHandle, OwnerToken, JsObjectHandle) {
        let mut session = RuntimeSession::new(SymbolOwner {
            session: session_id,
            generation: 1,
        });
        assert!(session.add_player(1, channel::unbounded().0));
        let session = session.into_handle();
        let object = object();
        object.borrow_mut().set_own(
            "echo",
            JsValue::Native(Rc::new(crate::player::js_lingo::value::NativeFn {
                name: "echo",
                call: Box::new(|args| Ok(args.first().cloned().unwrap_or(JsValue::Undefined))),
            })),
        );
        object.borrow_mut().set_own("value", JsValue::Int(7));
        let (owner, handle) = session
            .borrow_mut()
            .with_player_js(1, |context, registry| {
                let owner = context.player.owner.clone();
                let runtime = Rc::new(RefCell::new(JsRuntime::new()));
                let handle = registry
                    .register_object(1, &owner, &runtime, &object)
                    .expect("test object registration");
                (owner, handle)
            })
            .expect("test player exists");
        (session, owner, handle)
    }

    #[test]
    fn js_object_handles_are_owner_bound_when_numeric_ids_collide() {
        let owner_a = owner(1, 1);
        let owner_b = owner(1, 1);
        assert!(!owner_a.same_identity(&owner_b));
        let runtime_a = Rc::new(RefCell::new(JsRuntime::new()));
        let runtime_b = Rc::new(RefCell::new(JsRuntime::new()));
        let mut registry_a = JsRuntimeRegistry::default();
        let mut registry_b = JsRuntimeRegistry::default();
        let handle_a = registry_a
            .register_object(1, &owner_a, &runtime_a, &object())
            .expect("first owner can register an object");
        let handle_b = registry_b
            .register_object(1, &owner_b, &runtime_b, &object())
            .expect("second owner can register the same numeric coordinate");

        assert_eq!(handle_a.id(), handle_b.id());
        assert_ne!(handle_a, handle_b);
        assert!(registry_a.object(1, &owner_a, &handle_a).is_some());
        assert!(registry_b.object(1, &owner_b, &handle_b).is_some());
        assert!(registry_a.object(1, &owner_b, &handle_a).is_none());
        assert!(registry_b.object(1, &owner_a, &handle_b).is_none());
        assert!(registry_a.object(1, &owner_a, &handle_b).is_none());
        assert!(registry_b.object(1, &owner_b, &handle_a).is_none());
    }

    #[test]
    fn js_object_id_exhaustion_is_transactional() {
        let owner = owner(1, 1);
        let runtime = Rc::new(RefCell::new(JsRuntime::new()));
        let mut registry = JsRuntimeRegistry::default();
        registry.next_object_id = u32::MAX;
        let result = registry.register_object(1, &owner, &runtime, &object());

        assert!(result.is_err());
        assert_eq!(registry.next_object_id, u32::MAX);
        assert!(registry.objects.is_empty());
    }

    #[test]
    fn js_object_id_collision_is_transactional_and_same_object_reuses_id() {
        let owner = owner(1, 1);
        let runtime = Rc::new(RefCell::new(JsRuntime::new()));
        let first_object = object();
        let mut registry = JsRuntimeRegistry::default();
        let first = registry
            .register_object(1, &owner, &runtime, &first_object)
            .expect("first object can register");
        let counter_after_first = registry.next_object_id;
        let reused = registry
            .register_object(1, &owner, &runtime, &first_object)
            .expect("same object reuses its capability");
        assert_eq!(reused, first);
        assert_eq!(registry.next_object_id, counter_after_first);

        registry.next_object_id = 0;
        let second_object = object();
        let result = registry.register_object(1, &owner, &runtime, &second_object);
        assert!(result.is_err());
        assert_eq!(registry.next_object_id, 0);
        assert_eq!(registry.objects.len(), 1);
    }

    #[test]
    fn js_object_registration_rejects_stale_owner_before_mutation() {
        let owner = owner(1, 1);
        let runtime = Rc::new(RefCell::new(JsRuntime::new()));
        let mut registry = JsRuntimeRegistry::default();
        owner.begin_reset();
        let result = registry.register_object(1, &owner, &runtime, &object());

        assert!(result.is_err());
        assert_eq!(registry.next_object_id, 0);
        assert!(registry.objects.is_empty());
    }

    #[test]
    fn js_object_clear_drops_handles_and_new_owner_cannot_reuse_old_capability() {
        let old_owner = owner(1, 1);
        let new_owner = owner(1, 2);
        let runtime = Rc::new(RefCell::new(JsRuntime::new()));
        let mut registry = JsRuntimeRegistry::default();
        let old_object = object();
        let handle = registry
            .register_object(1, &old_owner, &runtime, &old_object)
            .expect("old owner can register an object");
        old_owner.begin_reset();
        registry.clear();

        assert!(registry.object(1, &old_owner, &handle).is_none());
        assert!(registry.object(1, &new_owner, &handle).is_none());
        assert!(registry.objects.is_empty());

        let fresh = registry
            .register_object(1, &new_owner, &runtime, &object())
            .expect("fresh owner can reuse the cleared numeric coordinate");
        assert_eq!(fresh.id(), handle.id());
        assert_ne!(fresh, handle);
        assert!(registry.object(1, &new_owner, &fresh).is_some());
        assert!(registry.object(1, &new_owner, &handle).is_none());
    }

    #[test]
    fn js_object_player_reset_clears_registry_and_reuses_id_safely() {
        let (session, old_owner, stale_handle) = session_with_object(304);
        let new_owner = session
            .borrow_mut()
            .reset_player_owned(1, &old_owner)
            .expect("the captured player owner can reset");
        assert!(!old_owner.same_identity(&new_owner));

        let (fresh_owner, fresh_handle) = session
            .borrow_mut()
            .with_player_js(1, |context, registry| {
                let runtime = Rc::new(RefCell::new(JsRuntime::new()));
                let fresh_object = object();
                fresh_object.borrow_mut().set_own("value", JsValue::Int(11));
                let handle = registry
                    .register_object(1, &context.player.owner, &runtime, &fresh_object)
                    .expect("the replacement owner can register an object");
                (context.player.owner.clone(), handle)
            })
            .expect("the replacement player exists");

        assert_eq!(stale_handle.id(), fresh_handle.id());
        assert!(get_js_object_prop_explicit(
            &session,
            1,
            &old_owner,
            &stale_handle,
            "value",
        )
        .is_err());
        let value = get_js_object_prop_explicit(
            &session,
            1,
            &fresh_owner,
            &fresh_handle,
            "value",
        )
        .expect("the fresh handle remains usable");
        assert!(session
            .borrow_mut()
            .with_player(1, |context| matches!(context.player.get_datum(&value), Datum::Int(11)))
            .expect("the replacement player exists"));
    }

    #[test]
    fn js_object_property_key_prefers_exact_case_before_lingo_fallback() {
        let mut object = crate::player::js_lingo::value::JsObject::new();
        object.set_own("foo", JsValue::Int(1));
        object.set_own("Foo", JsValue::Int(2));

        assert_eq!(resolve_js_object_key(&object, "Foo"), "Foo");
        assert_eq!(resolve_js_object_key(&object, "FOO"), "Foo");
    }

    #[test]
    fn js_object_property_lookup_preserves_exact_and_prototype_resolution() {
        let mut prototype = crate::player::js_lingo::value::JsObject::new();
        prototype.set_own("Inherited", JsValue::Int(13));
        let mut object = crate::player::js_lingo::value::JsObject::new();
        object.proto = Some(Rc::new(RefCell::new(prototype)));
        object.set_own("foo", JsValue::Int(1));
        object.set_own("Foo", JsValue::Int(2));
        let object = Rc::new(RefCell::new(object));

        assert!(matches!(resolve_js_object_prop(&object, "Foo"), Some(JsValue::Int(2))));
        assert!(matches!(resolve_js_object_prop(&object, "FOO"), Some(JsValue::Int(2))));
        assert!(matches!(resolve_js_object_prop(&object, "inherited"), Some(JsValue::Int(13))));
    }

    #[test]
    fn explicit_object_call_get_set_use_real_owner_bound_sessions() {
        let (session_a, owner_a, handle_a) = session_with_object(301);
        let (session_b, owner_b, handle_b) = session_with_object(301);

        let value = get_js_object_prop_explicit(
            &session_a,
            1,
            &owner_a,
            &handle_a,
            "value",
        )
        .expect("owner A can read its object");
        assert!(session_a
            .borrow_mut()
            .with_player(1, |context| matches!(context.player.get_datum(&value), Datum::Int(7)))
            .expect("owner A player exists"));

        let set_value = session_a
            .borrow_mut()
            .with_player(1, |context| context.player.alloc_datum(Datum::Int(9)))
            .expect("owner A player exists");
        set_js_object_prop_explicit(
            &session_a,
            1,
            &owner_a,
            &handle_a,
            "VALUE",
            &set_value,
        )
        .expect("owner A can set its object");

        let echoed = invoke_js_object_method_explicit(
            &session_a,
            1,
            &owner_a,
            &handle_a,
            "echo",
            &[set_value.clone()],
        )
        .expect("owner A can call its object");
        assert!(session_a
            .borrow_mut()
            .with_player(1, |context| matches!(context.player.get_datum(&echoed), Datum::Int(9)))
            .expect("owner A player exists"));

        assert!(get_js_object_prop_explicit(&session_b, 1, &owner_b, &handle_a, "value")
            .is_err(), "owner B cannot read owner A's colliding numeric handle");
        assert!(set_js_object_prop_explicit(
            &session_b,
            1,
            &owner_b,
            &handle_a,
            "value",
            &set_value,
        )
        .is_err(), "owner B cannot mutate owner A's colliding numeric handle");
        assert!(invoke_js_object_method_explicit(
            &session_b,
            1,
            &owner_b,
            &handle_a,
            "echo",
            &[DatumRef::Void],
        )
        .is_err(), "owner B cannot call owner A's colliding numeric handle");
        assert!(get_js_object_prop_explicit(&session_b, 1, &owner_b, &handle_b, "value").is_ok());
    }

    #[test]
    fn explicit_object_set_rejects_nested_foreign_value_before_mutation() {
        let (session_a, owner_a, handle_a) = session_with_object(307);
        let (session_b, owner_b, handle_b) = session_with_object(307);
        let foreign = session_a
            .borrow_mut()
            .with_player(1, |context| context.player.alloc_datum(Datum::JsObjectRef(handle_b)))
            .expect("owner A player exists");
        let nested = session_a
            .borrow_mut()
            .with_player(1, |context| {
                context.player.alloc_datum(Datum::List(
                    crate::director::lingo::datum::DatumType::List,
                    std::collections::VecDeque::from([foreign]),
                    false,
                ))
            })
            .expect("owner A player exists");

        let error = set_js_object_prop_explicit(
            &session_a,
            1,
            &owner_a,
            &handle_a,
            "value",
            &nested,
        )
        .expect_err("nested foreign values must fail preflight");
        assert_eq!(error.code, crate::player::ScriptErrorCode::InvalidReference);
        let value = get_js_object_prop_explicit(
            &session_a,
            1,
            &owner_a,
            &handle_a,
            "value",
        )
        .expect("the original property remains readable");
        assert!(session_a
            .borrow_mut()
            .with_player(1, |context| matches!(context.player.get_datum(&value), Datum::Int(7)))
            .expect("owner A player exists"));
    }

    #[test]
    fn explicit_object_call_rejects_cyclic_array_and_plain_object_results() {
        let (session, owner, handle) = session_with_object(302);
        let object = session
            .borrow()
            .js_lingo_registry()
            .object(1, &owner, &handle)
            .expect("registered test object")
            .1;
        let array = Rc::new(RefCell::new(crate::player::js_lingo::value::JsArray::new()));
        array.borrow_mut().items.push(JsValue::Array(array.clone()));
        let cyclic_object = Rc::new(RefCell::new(
            crate::player::js_lingo::value::JsObject::new(),
        ));
        cyclic_object
            .borrow_mut()
            .set_own("self", JsValue::Object(cyclic_object.clone()));
        let array_for_call = array.clone();
        object.borrow_mut().set_own(
            "cycleArray",
            JsValue::Native(Rc::new(crate::player::js_lingo::value::NativeFn {
                name: "cycleArray",
                call: Box::new(move |_| Ok(JsValue::Array(array_for_call.clone()))),
            })),
        );
        let object_for_call = cyclic_object.clone();
        object.borrow_mut().set_own(
            "cycleObject",
            JsValue::Native(Rc::new(crate::player::js_lingo::value::NativeFn {
                name: "cycleObject",
                call: Box::new(move |_| Ok(JsValue::Object(object_for_call.clone()))),
            })),
        );

        for handler in ["cycleArray", "cycleObject"] {
            let error = invoke_js_object_method_explicit(
                &session,
                1,
                &owner,
                &handle,
                handler,
                &[],
            )
            .expect_err("cyclic JS result must fail conversion");
            assert!(error.message.contains("cyclic JS"));
        }
    }

    #[test]
    fn explicit_object_call_allows_weak_same_runtime_reentry() {
        let (session, owner, handle) = session_with_object(305);
        let object = session
            .borrow()
            .js_lingo_registry()
            .object(1, &owner, &handle)
            .expect("registered test object")
            .1;
        object.borrow_mut().set_own("nested", JsValue::Int(19));
        object.borrow_mut().set_own(
            "nestedCall",
            JsValue::Native(Rc::new(crate::player::js_lingo::value::NativeFn {
                name: "nestedCall",
                call: Box::new(|_| Ok(JsValue::Int(21))),
            })),
        );
        let weak_session = Rc::downgrade(&session);
        let callback_owner = owner.clone();
        let callback_handle = handle.clone();
        object.borrow_mut().set_own(
            "reenter",
            JsValue::Native(Rc::new(crate::player::js_lingo::value::NativeFn {
                name: "reenter",
                call: Box::new(move |_| {
                    let session = weak_session
                        .upgrade()
                        .ok_or_else(|| JsError::new("Director session disposed"))?;
                    invoke_js_object_method_explicit(
                        &session,
                        1,
                        &callback_owner,
                        &callback_handle,
                        "nestedCall",
                        &[],
                    )
                    .map_err(|error| JsError::new(error.message))?;
                    Ok(JsValue::Int(23))
                }),
            })),
        );

        let result = invoke_js_object_method_explicit(
            &session,
            1,
            &owner,
            &handle,
            "reenter",
            &[],
        )
        .expect("same-runtime weak reentry succeeds");
        assert!(session
            .borrow_mut()
            .with_player(1, |context| matches!(context.player.get_datum(&result), Datum::Int(23)))
            .expect("the player exists"));
    }

    #[test]
    fn explicit_object_call_round_trips_real_script_proxy_and_reenters_runtime() {
        let (session, owner, handle) = session_with_object(309);
        let (runtime, object) = session
            .borrow()
            .js_lingo_registry()
            .object(1, &owner, &handle)
            .expect("registered test object");
        let member_ref = CastMemberRef { cast_lib: 2, cast_member: 9 };
        let proxy_method = |name: &str, property: &str| {
            Rc::new(crate::player::js_lingo::xdr::JsFunctionAtom {
                name: Some(name.to_owned()),
                nargs: 0,
                extra: 0,
                nvars: 0,
                flags: 0,
                bindings: Vec::new(),
                script: crate::player::js_lingo::xdr::JsScriptIR {
                    magic: 0,
                    bytecode: vec![
                        crate::player::js_lingo::opcodes::JsOp::This as u8,
                        crate::player::js_lingo::opcodes::JsOp::Getprop as u8,
                        0,
                        0,
                        crate::player::js_lingo::opcodes::JsOp::Return as u8,
                    ],
                    prolog_length: 0,
                    version: 150,
                    atoms: vec![crate::player::js_lingo::xdr::JsAtom::String(property.to_owned())],
                    source_notes: Vec::new(),
                    filename: None,
                    lineno: 1,
                    max_stack_depth: 2,
                    try_notes: Vec::new(),
                },
            })
        };
        runtime.borrow().global.borrow_mut().set_own(
            "ping",
            JsValue::Function(Rc::new(crate::player::js_lingo::value::JsFunction {
                atom: proxy_method("ping", "__script_member__"),
                captured_scope: None,
            })),
        );
        runtime.borrow().global.borrow_mut().set_own(
            "work",
            JsValue::Function(Rc::new(crate::player::js_lingo::value::JsFunction {
                atom: proxy_method("work", "__script_lib__"),
                captured_scope: None,
            })),
        );
        session
            .borrow_mut()
            .js_lingo_registry_mut()
            .insert_runtime(1, owner.clone(), member_ref.clone(), runtime.clone());
        object.borrow_mut().set_own(
            "forward",
            JsValue::Native(Rc::new(crate::player::js_lingo::value::NativeFn {
                name: "forward",
                call: Box::new(|args| Ok(args.first().cloned().unwrap_or(JsValue::Undefined))),
            })),
        );
        let script_ref = session
            .borrow_mut()
            .with_player(1, |context| context.player.alloc_datum(Datum::ScriptRef(member_ref)))
            .expect("the player exists");
        let proxy = invoke_js_object_method_explicit(
            &session,
            1,
            &owner,
            &handle,
            "forward",
            &[script_ref],
        )
        .expect("script proxy argument and result cross the owner boundary");
        let proxy_handle = session
            .borrow_mut()
            .with_player(1, |context| match context.player.get_datum(&proxy) {
                Datum::JsObjectRef(handle) => Some(handle.clone()),
                _ => None,
            })
            .expect("the player exists")
            .expect("the script proxy retains its callable handler");
        let result = invoke_js_object_method_explicit(
            &session,
            1,
            &owner,
            &proxy_handle,
            "ping",
            &[],
        )
        .expect("the real script proxy reenters its runtime");
        assert!(session
            .borrow_mut()
            .with_player(1, |context| matches!(context.player.get_datum(&result), Datum::Int(9)))
            .expect("the player exists"));
        let work = invoke_js_object_method_explicit(
            &session,
            1,
            &owner,
            &proxy_handle,
            "work",
            &[],
        )
        .expect("the outer interpreted proxy method reenters its runtime");
        assert!(session
            .borrow_mut()
            .with_player(1, |context| matches!(context.player.get_datum(&work), Datum::Int(2)))
            .expect("the player exists"));
        let result_again = invoke_js_object_method_explicit(
            &session,
            1,
            &owner,
            &proxy_handle,
            "ping",
            &[],
        )
        .expect("a subsequent interpreted proxy invocation remains usable");
        assert!(session
            .borrow_mut()
            .with_player(1, |context| matches!(context.player.get_datum(&result_again), Datum::Int(9)))
            .expect("the player exists"));
    }

    #[test]
    fn explicit_object_call_fences_reset_before_converting_late_result() {
        let (session, owner, handle) = session_with_object(306);
        let object = session
            .borrow()
            .js_lingo_registry()
            .object(1, &owner, &handle)
            .expect("registered test object")
            .1;
        let callback_owner = owner.clone();
        object.borrow_mut().set_own(
            "resetAndThrow",
            JsValue::Native(Rc::new(crate::player::js_lingo::value::NativeFn {
                name: "resetAndThrow",
                call: Box::new(move |_| {
                    callback_owner.begin_reset();
                    Err(JsError::new("callback failure after reset"))
                }),
            })),
        );

        let error = invoke_js_object_method_explicit(
            &session,
            1,
            &owner,
            &handle,
            "resetAndThrow",
            &[],
        )
        .expect_err("reset must fence the late callback result");
        assert_eq!(error.code, crate::player::ScriptErrorCode::InvalidReference);
    }

    #[test]
    fn owner_scheduler_completes_nested_value_operands_lists_and_js_globals() {
        let (session, owner, handle) = session_with_object(310);
        let object_ref = session
            .borrow_mut()
            .with_player(1, |context| {
                context
                    .player
                    .alloc_datum(Datum::JsObjectRef(handle.clone()))
            })
            .expect("the player exists");
        session
            .borrow_mut()
            .with_player(1, |context| {
                context.player.globals.insert(context.symbols.intern("g"), object_ref);
            })
            .expect("the player exists");

        let evaluate = |source: &str, mode: crate::player::driver::ValueEvaluationMode| {
            let source_ref = session
                .borrow_mut()
                .with_player(1, |context| {
                    context.player.alloc_datum(Datum::String(source.to_owned()))
                })
                .expect("the player exists");
            let invoke = crate::player::eval::invoke_value_request_owned(
                session.clone(),
                1,
                owner.clone(),
                source_ref,
                mode,
            );
            let drive = crate::player::commands::drive_pending_owner(&session, 1, &owner);
            let (result, ()) = futures::executor::block_on(async { futures::join!(invoke, drive) });
            result.expect("owner scheduler should complete the value request")
        };

        let arithmetic = evaluate(
            "1 + \"2\".value",
            crate::player::driver::ValueEvaluationMode::GlobalVoid,
        );
        let list = evaluate(
            "[\"2\".value, 3]",
            crate::player::driver::ValueEvaluationMode::GlobalVoid,
        );
        let global = evaluate(
            "g.value",
            crate::player::driver::ValueEvaluationMode::GlobalVoid,
        );
        let global_fallback = evaluate(
            "g.value",
            crate::player::driver::ValueEvaluationMode::StringPropertyFallback,
        );
        let values = session
            .borrow_mut()
            .with_player(1, |context| {
                (
                    context.player.get_datum(&arithmetic).clone(),
                    context.player.get_datum(&list).clone(),
                    context.player.get_datum(&global).clone(),
                    context.player.get_datum(&global_fallback).clone(),
                )
            })
            .expect("the player exists");
        assert!(matches!(values.0, Datum::Int(3)));
        let Datum::List(_, entries, _) = values.1 else {
            panic!("expected a nested list value");
        };
        assert_eq!(entries.len(), 2);
        session.borrow_mut().with_player(1, |context| {
            assert!(matches!(context.player.get_datum(&entries[0]), Datum::Int(2)));
            assert!(matches!(context.player.get_datum(&entries[1]), Datum::Int(3)));
        });
        assert!(matches!(values.2, Datum::Int(7)));
        assert!(matches!(values.3, Datum::Int(7)));

        // Drive an indexed assignment through the owner-bound evaluator and
        // then fetch the property through a fresh JS request. This exercises
        // the production ApplyIndexedAssignment continuation, including its
        // pending get/set pair, rather than only inspecting a unit turn.
        let first = session
            .borrow_mut()
            .with_player(1, |context| context.player.alloc_datum(Datum::Int(1)))
            .expect("the player exists");
        let second = session
            .borrow_mut()
            .with_player(1, |context| context.player.alloc_datum(Datum::Int(2)))
            .expect("the player exists");
        let items = session
            .borrow_mut()
            .with_player(1, |context| {
                context.player.alloc_datum(Datum::List(
                    crate::director::lingo::datum::DatumType::List,
                    std::collections::VecDeque::from([first, second]),
                    false,
                ))
            })
            .expect("the player exists");
        set_js_object_prop_explicit(&session, 1, &owner, &handle, "items", &items)
            .expect("initial items property should be writable");
        let assignment = crate::player::eval_lingo_command_owned(
            session.clone(),
            1,
            owner.clone(),
            "g.items[1] = 9".to_owned(),
        );
        let scheduler = crate::player::commands::drive_pending_owner(&session, 1, &owner);
        let (assignment, ()) = futures::executor::block_on(async {
            futures::join!(assignment, scheduler)
        });
        assignment.expect("indexed JS assignment should complete through the owner scheduler");
        let updated = get_js_object_prop_explicit(&session, 1, &owner, &handle, "items")
            .expect("updated items property should be readable");
        let updated = session
            .borrow_mut()
            .with_player(1, |context| context.player.get_datum(&updated).clone())
            .expect("the player exists");
        let Datum::List(_, updated, _) = updated else {
            panic!("updated items property should remain a list");
        };
        assert_eq!(updated.len(), 2);
        assert!(matches!(
            session
                .borrow_mut()
                .with_player(1, |context| context.player.get_datum(&updated[0]).clone()),
            Some(Datum::Int(9))
        ));
        assert!(matches!(
            session
                .borrow_mut()
                .with_player(1, |context| context.player.get_datum(&updated[1]).clone()),
            Some(Datum::Int(2))
        ));
        let session_ref = session.borrow();
        assert!(!session_ref.has_pending_eval_requests(1));
        assert!(!session_ref.has_pending_commands(1));
    }

    #[test]
    fn owner_scheduler_evaluates_interpreted_factory_global_value_routes() {
        let (session, owner, handle) = session_with_object(311);
        let object = session
            .borrow()
            .js_lingo_registry()
            .object(1, &owner, &handle)
            .expect("registered test object")
            .1;
        object.borrow_mut().set_own("x", JsValue::Int(17));
        object.borrow_mut().set_own(
            "factory",
            JsValue::Function(Rc::new(crate::player::js_lingo::value::JsFunction {
                atom: Rc::new(crate::player::js_lingo::xdr::JsFunctionAtom {
                    name: Some("factory".to_owned()),
                    nargs: 0,
                    extra: 0,
                    nvars: 0,
                    flags: 0,
                    bindings: Vec::new(),
                    script: crate::player::js_lingo::xdr::JsScriptIR {
                        magic: 0,
                        bytecode: vec![
                            crate::player::js_lingo::opcodes::JsOp::This as u8,
                            crate::player::js_lingo::opcodes::JsOp::Return as u8,
                        ],
                        prolog_length: 0,
                        version: 150,
                        atoms: Vec::new(),
                        source_notes: Vec::new(),
                        filename: None,
                        lineno: 1,
                        max_stack_depth: 1,
                        try_notes: Vec::new(),
                    },
                }),
                captured_scope: None,
            })),
        );

        let h_ref = session
            .borrow_mut()
            .with_player(1, |context| {
                let value = context.player.alloc_datum(Datum::JsObjectRef(handle.clone()));
                context.player.globals.insert(context.symbols.intern("h"), value.clone());
                value
            })
            .expect("the player exists");
        let _ = h_ref;

        let run_command = |source: &str| {
            let command = crate::player::eval_lingo_command_owned(
                session.clone(),
                1,
                owner.clone(),
                source.to_owned(),
            );
            let scheduler = crate::player::commands::drive_pending_owner(&session, 1, &owner);
            let (result, ()) = futures::executor::block_on(async {
                futures::join!(command, scheduler)
            });
            result.expect("owner scheduler should complete the command")
        };

        run_command("g = h.factory()");
        let returned_handle = session
            .borrow_mut()
            .with_player(1, |context| {
                let value = context.player.globals.get(&context.symbols.intern("g"))?;
                match context.player.get_datum(value) {
                    Datum::JsObjectRef(handle) => Some(handle.clone()),
                    _ => None,
                }
            })
            .expect("the player exists")
            .expect("factory result should be retained as a JS object handle");
        assert_eq!(returned_handle, handle);

        run_command("propertyResult = \"g.x\".value");
        run_command("globalResult = value(\"g.x\")");

        let results = session
            .borrow_mut()
            .with_player(1, |context| {
                let property = context.symbols.intern("propertyResult");
                let global = context.symbols.intern("globalResult");
                (
                    context.player.globals.get(&property).map(|value| context.player.get_datum(value).clone()),
                    context.player.globals.get(&global).map(|value| context.player.get_datum(value).clone()),
                )
            })
            .expect("the player exists");
        assert!(matches!(results.0, Some(Datum::Int(17))));
        assert!(matches!(results.1, Some(Datum::Int(17))));
        let session_ref = session.borrow();
        assert!(!session_ref.has_pending_eval_requests(1));
        assert!(!session_ref.has_pending_commands(1));
    }
}
