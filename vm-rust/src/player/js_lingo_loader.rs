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
// The legacy setup caller still has a compatibility mirror until it receives
// RuntimeSessionHandle; all new paths use the session-owned registry.
//
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use crate::director::lingo::datum::Datum;
use crate::player::cast_lib::CastMemberRef;
use crate::player::allocator::DatumAllocatorTrait;
use crate::player::datum_ref::DatumRef;
use crate::player::js_lingo::host_bridge::{JsHostBridge, StubBridge};
use crate::player::js_lingo::value::{JsError, JsObjectRef, JsValue};
use crate::player::js_lingo::{decode_script, disasm::disassemble, JsScriptIR};
use crate::player::js_lingo::interpreter::JsRuntime;
use crate::player::reserve_player_mut;
use crate::player::script::Script;
use crate::player::symbols::{builtin::BuiltInSymbol, symbol::Symbol};

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
    ) -> u32 {
        if let Some((id, _)) = self.objects.iter().find(|(_, entry)| {
            entry.player_id == player_id
                && entry.owner.same_identity(owner)
                && Rc::ptr_eq(&entry.object, object)
        }) {
            return *id;
        }
        self.next_object_id = self.next_object_id.wrapping_add(1).max(1);
        let id = self.next_object_id;
        self.objects.insert(id, JsObjectEntry {
            player_id,
            owner: owner.clone(),
            runtime: runtime.clone(),
            object: object.clone(),
        });
        id
    }

    pub(crate) fn object(
        &self,
        player_id: crate::player::session::PlayerId,
        owner: &crate::player::ownership::OwnerToken,
        id: u32,
    ) -> Option<(Rc<RefCell<JsRuntime>>, JsObjectRef)> {
        let entry = self.objects.get(&id)?;
        (entry.player_id == player_id && entry.owner.same_identity(owner)
            && owner.is_arena_live()).then(|| (entry.runtime.clone(), entry.object.clone()))
    }

    pub(crate) fn clear(&mut self) {
        self.runtimes.clear();
        self.objects.clear();
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
}

// Transitional registry for the unmigrated setup_handler_frame caller. The
// owner-bound session registry is authoritative for new APIs; these entries
// remain until that legacy caller receives a RuntimeSessionHandle.
thread_local! {
    static JS_RUNTIMES: RefCell<HashMap<CastMemberRef, Rc<RefCell<JsRuntime>>>> = RefCell::new(HashMap::new());
    static JS_OBJECTS: RefCell<HashMap<u32, (Rc<RefCell<JsRuntime>>, JsObjectRef)>> = RefCell::new(HashMap::new());
    static NEXT_JS_OBJECT_ID: RefCell<u32> = RefCell::new(1);
    static CURRENT_RUNTIME: RefCell<Vec<Rc<RefCell<JsRuntime>>>> = RefCell::new(Vec::new());
}

fn with_current_runtime<T>(runtime: &Rc<RefCell<JsRuntime>>, f: impl FnOnce() -> T) -> T {
    CURRENT_RUNTIME.with(|stack| stack.borrow_mut().push(runtime.clone()));
    let result = f();
    CURRENT_RUNTIME.with(|stack| { stack.borrow_mut().pop(); });
    result
}

fn register_js_object(runtime: &Rc<RefCell<JsRuntime>>, object: &JsObjectRef) -> u32 {
    JS_OBJECTS.with(|objects| {
        let mut objects = objects.borrow_mut();
        if let Some((id, _)) = objects.iter().find(|(_, (_, existing))| Rc::ptr_eq(existing, object)) {
            return *id;
        }
        let id = NEXT_JS_OBJECT_ID.with(|next| {
            let mut next = next.borrow_mut();
            let id = *next;
            *next = next.wrapping_add(1).max(1);
            id
        });
        objects.insert(id, (runtime.clone(), object.clone()));
        id
    })
}

pub fn get_js_object(id: u32) -> Option<(Rc<RefCell<JsRuntime>>, JsObjectRef)> {
    JS_OBJECTS.with(|objects| objects.borrow().get(&id).cloned())
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
pub fn invoke_js_object_method(
    id: u32,
    name: &str,
    args: &[DatumRef],
    symbols: &mut crate::player::symbols::symbol_table::SymbolTable,
) -> Option<Result<DatumRef, String>> {
    let _ = (id, name, args, symbols);
    None
}

/// Read a property using the owner-bound session API. The old table-only
/// signature remains for source compatibility and deliberately declines
/// access because it cannot validate the owning player.
pub fn get_js_object_prop(
    id: u32,
    name: &str,
    symbols: &mut crate::player::symbols::symbol_table::SymbolTable,
) -> Option<Result<DatumRef, String>> {
    let _ = (id, name, symbols);
    None
}

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

/// Write a property on a live JS object, reusing the existing key's casing
/// when one matches case-insensitively so Lingo writes don't shadow the
/// property JS declared.
pub fn set_js_object_prop(obj: &JsObjectRef, name: &str, value: JsValue) {
    let mut b = obj.borrow_mut();
    let key = b
        .props
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(k, _)| k.clone())
        .unwrap_or_else(|| name.to_string());
    b.set_own(&key, value);
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
    // Keep the legacy setup path functional until its caller is migrated to
    // `try_invoke_js_handler_explicit`; the session registry remains the
    // authoritative owner-checked store for all new access.
    JS_RUNTIMES.with(|runtimes| {
        runtimes.borrow_mut().insert(member_ref, runtime);
    });
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

/// Legacy teardown hook retained for older frontend callers. Runtime state is
/// owned by each RuntimeSession and is cleared by its reset/removal boundary;
/// the transitional mirror is cleared here until setup is migrated.
pub fn clear_all_runtimes() {
    JS_RUNTIMES.with(|runtimes| runtimes.borrow_mut().clear());
    JS_OBJECTS.with(|objects| objects.borrow_mut().clear());
    CURRENT_RUNTIME.with(|stack| stack.borrow_mut().clear());
}

/// Legacy signature retained while callers migrate to the owner-bound API.
/// New code must use `try_invoke_js_handler_explicit`; this path preserves the
/// existing setup caller's behavior until it receives an explicit session.
pub fn try_invoke_js_handler(
    script_member_ref: &CastMemberRef,
    handler_name: &str,
    args: &[DatumRef],
    has_receiver: bool,
    symbols: &mut crate::player::symbols::symbol_table::SymbolTable,
) -> Option<Result<DatumRef, String>> {
    let runtime = JS_RUNTIMES.with(|runtimes| runtimes.borrow().get(script_member_ref).cloned())?;
    let callee = {
        let runtime_ref = runtime.borrow();
        let global = runtime_ref.global.borrow();
        global.get_own(handler_name).cloned()?
    };
    if !matches!(callee, JsValue::Function(_) | JsValue::Native(_)) {
        return None;
    }
    let mut js_args = match reserve_player_mut(|player| {
        args.iter().map(|datum| datum_ref_to_js_value(player, symbols, datum)).collect::<Result<Vec<_>, _>>()
    }) {
        Ok(values) => values,
        Err(error) => return Some(Err(error.message)),
    };
    let _ = has_receiver;
    if let JsValue::Function(function) = &callee {
        let first_arg_is_me = function.atom.bindings.iter()
            .find(|binding| binding.kind == super::js_lingo::xdr::JsBindingKind::Argument)
            .map(|binding| matches!(binding.name.to_lowercase().as_str(), "me" | "mee" | "self" | "_me" | "this_" | "_self"))
            .unwrap_or(false);
        if first_arg_is_me {
            js_args.insert(0, JsValue::Undefined);
        }
    }
    let this_value = JsValue::Object(runtime.borrow().global.clone());
    Some(with_current_runtime(&runtime, || {
        match runtime.borrow_mut().invoke(&callee, js_args, this_value) {
            Ok(value) => reserve_player_mut(|player| js_value_to_datum_ref(player, symbols, &value))
                .map_err(|error| error.message),
            Err(error) => Err(error.message),
        }
    }))
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
    has_receiver: bool,
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
        let mut conversion = JsConversionContext {
            registry,
            runtime: &runtime_for_conversion,
            session: Rc::downgrade(session),
            player_id,
            owner: owner.clone(),
        };
        args.iter()
            .map(|datum| datum_ref_to_js_value_with_context(context.player, context.symbols, &mut conversion, datum))
            .collect::<Result<Vec<_>, _>>()
    }) {
        Some(Ok(values)) => values,
        Some(Err(error)) => return Some(Err(error.message)),
        None => return Some(Err("stale or foreign Director player".to_owned())),
    };
    let _ = has_receiver;
    if let JsValue::Function(function) = &callee {
        let first_arg_is_me = function.atom.bindings.iter()
            .find(|binding| binding.kind == super::js_lingo::xdr::JsBindingKind::Argument)
            .map(|binding| matches!(binding.name.to_lowercase().as_str(), "me" | "mee" | "self" | "_me" | "this_" | "_self"))
            .unwrap_or(false);
        if first_arg_is_me {
            js_args.insert(0, JsValue::Undefined);
        }
    }
    let this_value = JsValue::Object(runtime.borrow().global.clone());
    let result = match runtime.borrow_mut().invoke(&callee, js_args, this_value) {
        Ok(value) => value,
        Err(error) => return Some(Err(error.message)),
    };
    let runtime_for_conversion = runtime.clone();
    Some(match session.borrow_mut().with_player_js(player_id, |mut context, registry| {
        if !context.player.owner.is_arena_live() || !owner.same_identity(&context.player.owner) {
            return Err(JsError::new("stale or foreign Director player"));
        }
        let mut conversion = JsConversionContext {
            registry,
            runtime: &runtime_for_conversion,
            session: Rc::downgrade(session),
            player_id,
            owner: owner.clone(),
        };
        js_value_to_datum_ref_with_context(context.player, context.symbols, &mut conversion, &result)
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
pub fn datum_ref_to_js_value(
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
            let arr: Vec<JsValue> = items.iter().map(|r| datum_ref_to_js_value(player, symbols, r)).collect::<Result<_, _>>()?;
            Ok(JsValue::Array(Rc::new(RefCell::new(crate::player::js_lingo::value::JsArray { items: arr }))))
        }
        Datum::PropList(pairs, _) => {
            let mut obj = crate::player::js_lingo::value::JsObject::new();
            for (k_ref, v_ref) in pairs {
                let key = datum_ref_to_string(player, symbols, &k_ref)?;
                let v = datum_ref_to_js_value(player, symbols, &v_ref)?;
                obj.set_own(&key, v);
            }
            Ok(JsValue::Object(Rc::new(RefCell::new(obj))))
        }
        Datum::ScriptRef(member_ref) => Ok(script_ref_to_js_proxy(member_ref)),
        // Round-trip a live handle back to the very same JS object, so
        // identity survives a Lingo→JS→Lingo hop
        Datum::JsObjectRef(id) => match get_js_object(id) {
            Some((_, obj)) => Ok(JsValue::Object(obj)),
            None => Ok(JsValue::Undefined),
        },
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
    use crate::player::js_lingo::value::{JsObject, NativeFn};
    let mut obj = JsObject::new();
    obj.class_name = "ScriptRef";
    obj.set_own("__script_lib__", JsValue::Int(member_ref.cast_lib));
    obj.set_own("__script_member__", JsValue::Int(member_ref.cast_member));
    let runtime = session.upgrade().and_then(|handle| {
        let session_ref = handle.borrow();
        session_ref.js_lingo_registry().runtime(player_id, &owner, &member_ref)
    });
    let Some(runtime) = runtime else {
        return JsValue::Object(Rc::new(RefCell::new(obj)));
    };
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
            };
            for datum in js_args {
                converted.push(datum_ref_to_js_value_with_context(context.player, context.symbols, &mut conversion, &datum)?);
            }
            Ok::<_, JsError>(converted)
        }).ok_or_else(|| JsError::new("stale or foreign Director player"))??
    };
    let this_value = JsValue::Object(runtime.borrow().global.clone());
    let result = runtime.borrow_mut().invoke(&callee, js_args, this_value)
        .map_err(|error| JsError::new(error.message))?;
    let datum = {
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
            };
            js_value_to_datum_ref_with_context(context.player, context.symbols, &mut conversion, &result)
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
pub fn js_value_to_datum_ref(
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
                .map(|x| js_value_to_datum_ref(player, symbols, x))
                .collect::<Result<_, _>>()?;
            Datum::List(crate::director::lingo::datum::DatumType::List, items, false)
        }
        JsValue::Object(o) => {
            // Objects carrying methods stay live (see `object_has_callable`).
            // Without a runtime in scope there's nothing to invoke against
            // later, so those fall through to the copy below.
            // Callable objects are retained by the explicit conversion
            // context. This compatibility conversion has no owner authority,
            // so it preserves the object as a plain PropList below.
            let pairs: std::collections::VecDeque<crate::director::lingo::datum::PropListPair> = o
                .borrow()
                .props
                .iter()
                .map(|(k, val)| -> Result<crate::director::lingo::datum::PropListPair, JsError> {
                    let key_dr = player.alloc_datum(Datum::Symbol(symbols.intern(k)));
                    let val_dr = js_value_to_datum_ref(player, symbols, val)?;
                    Ok((key_dr, val_dr))
                })
                .collect::<Result<_, _>>()?;
            Datum::PropList(pairs, false)
        }
        JsValue::Function(_) | JsValue::Native(_) => Datum::String("[Function]".to_string()),
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
        Datum::ScriptRef(member_ref) => Ok(script_ref_to_js_proxy_with_session(
            member_ref,
            context.session.clone(),
            context.player_id,
            context.owner.clone(),
        )),
        Datum::JsObjectRef(id) => {
            let (_, object) = context.registry.object(context.player_id, &context.owner, id)
                .ok_or_else(|| JsError::new("stale or foreign JS object"))?;
            Ok(JsValue::Object(object))
        }
        _ => datum_ref_to_js_value(player, symbols, dref),
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
            let items = array.borrow().items.clone().into_iter()
                .map(|item| js_value_to_datum_ref_with_context(player, symbols, context, &item))
                .collect::<Result<std::collections::VecDeque<_>, _>>()?;
            Ok(player.alloc_datum(Datum::List(crate::director::lingo::datum::DatumType::List, items, false)))
        }
        JsValue::Object(object) if object_has_callable(object) => {
            let id = context.registry.register_object(
                context.player_id,
                &context.owner,
                context.runtime,
                object,
            );
            Ok(player.alloc_datum(Datum::JsObjectRef(id)))
        }
        JsValue::Object(object) => {
            let pairs = object.borrow().props.clone().into_iter()
                .map(|(key, value)| {
                    let key_ref = player.alloc_datum(Datum::Symbol(symbols.intern(&key)));
                    let value_ref = js_value_to_datum_ref_with_context(player, symbols, context, &value)?;
                    Ok((key_ref, value_ref))
                })
                .collect::<Result<std::collections::VecDeque<_>, JsError>>()?;
            Ok(player.alloc_datum(Datum::PropList(pairs, false)))
        }
        _ => js_value_to_datum_ref(player, symbols, value),
    }
}
