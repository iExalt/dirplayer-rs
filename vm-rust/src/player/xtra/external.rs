// SPDX-License-Identifier: GPL-3.0-only
//
//! Host-side adapter for external Xtras (WASM plugins loaded from URLs).
//!
//! ## Flow
//!
//! ```text
//! Lingo: new(xtra "BobbaXtra")
//!   └─ manager.rs::create_xtra_instance
//!        └─ session::TypeNewPlan::Xtra
//!             └─ external::prepare_create_request(...)
//!                  └─ owner executor: execute_request(...)
//!                       └─ JS bridge: createExternalXtraInstance(...)
//!                            └─ plugin export: __xtra_create_instance(...)
//! ```
//!
//! ## Adding a new host capability (the scaling rule)
//!
//! 1. Append a variant to [`HostOp`] (never renumber existing entries).
//! 2. Add the handler arm in [`host_call_dispatch`].
//! 3. Add the matching Rust wrapper in `dirplayer-xtra`'s `host_env.rs`.
//!
//! Three places. No JS-side change required (the JS dispatcher is a pure
//! passthrough that doesn't decode postcard).

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::atomic::{AtomicU64, Ordering},
};

use futures::channel::oneshot;

use crate::director::lingo::datum::{Datum, DatumType, XtraInstanceId, datum_bool};
use crate::player::{
    DatumRef, DirPlayer, ScriptError,
    ownership::OwnerToken,
    symbols::symbol::Symbol,
    symbols::symbol_table::SymbolTable,
};

use xtra_sdk::Datum as XDatum;
use xtra_sdk::scene3d::{FrameData as SceneFrameData, MeshData as SceneMeshData};
use xtra_sdk::wire;


/// Discriminator for the single `dx_host_call` extern that every plugin
/// imports. Must match `HostOp` in `dirplayer-xtra/src/host_env.rs` and
/// the JS-side passthrough in `src/services/externalXtras.ts`.
///
/// **APPEND-ONLY. Never renumber.** Plugins built against older SDK
/// versions assume these numbers are stable.
#[repr(u32)]
#[derive(Debug, Clone, Copy)]
enum HostOp {
    Log = 1,
    RandomFill = 2,
    StorageGet = 3,
    StorageSet = 4,
    CreateXtraInstance = 5,
    CallXtraHandler = 6,
    DestroyXtraInstance = 7,
    Scene3dCreate = 8,
    Scene3dUploadMesh = 9,
    Scene3dDropMesh = 10,
    Scene3dUploadTexture = 11,
    Scene3dSubmitFrame = 12,
    Scene3dDestroy = 13,
    CastMemberBytes = 14,
    StageInfo = 15,
    MouseLoc = 16,
    KeyDown = 17,
    SetLingoGlobal = 18,
    MemberSize = 19,
}

impl HostOp {
    fn from_u32(v: u32) -> Option<Self> {
        Some(match v {
            1 => HostOp::Log,
            2 => HostOp::RandomFill,
            3 => HostOp::StorageGet,
            4 => HostOp::StorageSet,
            5 => HostOp::CreateXtraInstance,
            6 => HostOp::CallXtraHandler,
            7 => HostOp::DestroyXtraInstance,
            8 => HostOp::Scene3dCreate,
            9 => HostOp::Scene3dUploadMesh,
            10 => HostOp::Scene3dDropMesh,
            11 => HostOp::Scene3dUploadTexture,
            12 => HostOp::Scene3dSubmitFrame,
            13 => HostOp::Scene3dDestroy,
            14 => HostOp::CastMemberBytes,
            15 => HostOp::StageInfo,
            16 => HostOp::MouseLoc,
            17 => HostOp::KeyDown,
            18 => HostOp::SetLingoGlobal,
            19 => HostOp::MemberSize,
            _ => return None,
        })
    }
}

// ── Registry ─────────────────────────────────────────────────────────────

/// A prepared external call contains only owned data. It may cross the
/// session/host boundary, but its completion must be applied only to the
/// exact owner captured here.
#[derive(Clone)]
pub(crate) struct ExternalXtraRequest {
    pub(crate) owner: OwnerToken,
    pub(crate) owner_key: String,
    pub(crate) xtra_name: String,
    pub(crate) operation: ExternalXtraOperation,
    pub(crate) args: Vec<u8>,
}

#[derive(Clone)]
pub(crate) enum ExternalXtraOperation {
    ProbeStatic {
        handler: String,
        candidates: Vec<String>,
        raw_args: Vec<DatumRef>,
        fallback_name: Symbol,
        fallback_args: Vec<DatumRef>,
    },
    Static { handler: String },
    Instance { instance_id: XtraInstanceId, handler: String },
    Create,
    Destroy { instance_id: XtraInstanceId },
}

/// The operation that was suspended while an on-demand xtra was loading.
/// This is deliberately kept in owner-local state rather than in the host
/// request: the request is a cloneable capability, while the continuation is
/// consumed exactly once by the executor after the load completion arrives.
#[derive(Clone)]
pub(crate) enum ExternalXtraContinuation {
    Static {
        xtra_name: String,
        handler: String,
        args: Vec<u8>,
    },
    Instance {
        xtra_name: String,
        instance_id: XtraInstanceId,
        handler: String,
        args: Vec<u8>,
    },
    Create {
        xtra_name: String,
        args: Vec<u8>,
    },
}

/// Host bytes paired with the exact plugin that produced them. Bare static
/// probes cannot derive this name from the prepared request, so the selected
/// identity must cross the execute/finish boundary explicitly.
pub(crate) struct ExternalXtraResponse {
    pub(crate) xtra_name: String,
    pub(crate) bytes: Vec<u8>,
}

/// A cloneable on-demand load capability. The receiver remains in the
/// owner-local state and is taken exactly once by the executor.
#[derive(Clone)]
pub(crate) struct ExternalXtraLoadRequest {
    pub(crate) owner: OwnerToken,
    pub(crate) owner_key: String,
    pub(crate) name: String,
    pub(crate) request_id: u64,
    pub(crate) state_id: u64,
    pub(crate) notify_host: bool,
}

impl ExternalXtraLoadRequest {
    /// Opaque capability carried through the browser callback.  The state
    /// nonce prevents two players whose request counters both start at one
    /// from satisfying one another's completion.
    pub(crate) fn attempt_capability(&self) -> String {
        format!("{}:{}", self.state_id, self.request_id)
    }
}

/// Set of xtra names (lowercased) currently registered as externally
/// loaded. The JS-side loader calls [`register`] right after a successful
/// `WebAssembly.instantiate`, and the dispatch arms in `manager.rs`
/// consult this set before falling through to built-ins.
pub(crate) struct ExternalXtraState {
    state_id: u64,
    registered: HashSet<String>,
    pending_loads: HashMap<String, PendingLoad>,
    completed_receivers: HashMap<u64, RetainedReceiver>,
    continuations: HashMap<u64, PendingContinuation>,
    next_request_id: u64,
}

impl Default for ExternalXtraState {
    fn default() -> Self {
        Self {
            state_id: NEXT_EXTERNAL_STATE_ID.fetch_add(1, Ordering::Relaxed),
            registered: HashSet::new(),
            pending_loads: HashMap::new(),
            completed_receivers: HashMap::new(),
            continuations: HashMap::new(),
            next_request_id: 1,
        }
    }
}

static NEXT_EXTERNAL_STATE_ID: AtomicU64 = AtomicU64::new(1);

impl ExternalXtraState {
    pub(crate) fn register(&mut self, name: &str) {
        self.registered.insert(name.to_lowercase());
    }

    pub(crate) fn is_registered(&self, name: &str) -> bool {
        self.registered.contains(&name.to_lowercase())
    }

    pub(crate) fn registered_names(&self) -> Vec<String> {
        self.registered.iter().cloned().collect()
    }

    pub(crate) fn begin_load(
        &mut self,
        owner: OwnerToken,
        owner_key: String,
        name: &str,
    ) -> Result<ExternalXtraLoadRequest, ScriptError> {
        if !owner.is_arena_live() {
            return Err(ScriptError::new_code(
                crate::player::ScriptErrorCode::Abort,
                "external Xtra load owner was retired".to_owned(),
            ));
        }
        let request_id = self.next_request_id;
        let next_request_id = request_id.checked_add(1).ok_or_else(|| {
            ScriptError::new("external Xtra request-id space is exhausted".to_owned())
        })?;
        let key = name.to_lowercase();
        if let Some(entry) = self.pending_loads.get(&key) {
            if !entry.owner.same_identity(&owner) || !entry.owner.is_arena_live() {
                return Err(ScriptError::new_code(
                    crate::player::ScriptErrorCode::InvalidReference,
                    "external Xtra load owner does not match pending state".to_owned(),
                ));
            }
        }
        if self.completed_receivers.values().any(|retained| {
            retained.name == key && !retained.owner.same_identity(&owner)
        }) {
            return Err(ScriptError::new_code(
                crate::player::ScriptErrorCode::InvalidReference,
                "external Xtra load owner does not match completed state".to_owned(),
            ));
        }
        if request_id == 0
            || self.pending_loads.values().any(|entry| {
                entry.waiters.contains_key(&request_id)
                    || entry.receivers.contains_key(&request_id)
            })
            || self.completed_receivers.contains_key(&request_id)
            || self.continuations.contains_key(&request_id)
        {
            return Err(ScriptError::new(
                "external Xtra request-id space is exhausted".to_owned(),
            ));
        }
        let (tx, rx) = oneshot::channel::<Result<(), ScriptError>>();
        self.next_request_id = next_request_id;
        let entry = self.pending_loads.entry(key.clone()).or_insert_with(|| PendingLoad {
            owner: owner.clone(),
            waiters: HashMap::new(),
            receivers: HashMap::new(),
        });
        let first = entry.waiters.is_empty();
        entry.waiters.insert(request_id, tx);
        entry.receivers.insert(request_id, rx);
        Ok(ExternalXtraLoadRequest {
            owner,
            owner_key,
            name: key,
            request_id,
            state_id: self.state_id,
            notify_host: first,
        })
    }

    /// Retain the operation that will be retried after this capability's
    /// load completes. The opaque request ID is the only thing that crosses
    /// the host boundary; no oneshot receiver or player borrow is embedded.
    pub(crate) fn attach_continuation(
        &mut self,
        request: &ExternalXtraLoadRequest,
        continuation: ExternalXtraContinuation,
    ) -> Result<(), ScriptError> {
        if request.state_id != self.state_id || !request.owner.is_arena_live() {
            return Err(ScriptError::new_code(
                crate::player::ScriptErrorCode::Abort,
                "external Xtra load owner was retired".to_owned(),
            ));
        }
        let Some(entry) = self.pending_loads.get(&request.name) else {
            return Err(ScriptError::new(
                "external Xtra load request is no longer pending".to_owned(),
            ));
        };
        if !entry.owner.same_identity(&request.owner)
            || !entry.owner.is_arena_live()
            || !entry.waiters.contains_key(&request.request_id)
        {
            return Err(ScriptError::new(
                "external Xtra load request has no live waiter".to_owned(),
            ));
        }
        if self.continuations.contains_key(&request.request_id) {
            return Err(ScriptError::new(
                "external Xtra load continuation is already attached".to_owned(),
            ));
        }
        self.continuations.insert(
            request.request_id,
            PendingContinuation {
                owner: request.owner.clone(),
                continuation,
            },
        );
        Ok(())
    }

    pub(crate) fn complete_load(
        &mut self,
        owner: &OwnerToken,
        state_id: u64,
        request_id: u64,
        name: &str,
        success: bool,
    ) {
        if state_id != self.state_id {
            return;
        }
        let key = name.to_lowercase();
        let owner_matches = self
            .pending_loads
            .get(&key)
            .is_some_and(|entry| {
                entry.owner.same_identity(owner)
                    && owner.is_arena_live()
                    && entry.waiters.contains_key(&request_id)
            });
        if !owner_matches {
            return;
        }
        if success {
            self.registered.insert(key.clone());
        }
        let completion = self.pending_loads.remove(&key).map(|entry| {
            let request_ids: Vec<u64> = entry.waiters.keys().copied().collect();
            (request_ids, entry.waiters, entry.receivers, entry.owner)
        });
        if let Some((request_ids, waiters, receivers, owner)) = completion {
            for (_, tx) in waiters {
                let result = if success {
                    Ok(())
                } else {
                    Err(ScriptError::new(format!(
                        "external Xtra '{}' failed to load",
                        name
                    )))
                };
                let _ = tx.send(result);
            }
            for (request_id, receiver) in receivers {
                self.completed_receivers.insert(
                    request_id,
                    RetainedReceiver {
                        owner: owner.clone(),
                        name: key.clone(),
                        receiver,
                    },
                );
            }
            if !success {
                for request_id in request_ids {
                    self.continuations.remove(&request_id);
                }
            }
        }
    }

    pub(crate) fn cancel_pending_loads(&mut self) {
        let drained: Vec<PendingLoad> = self.pending_loads.drain().map(|(_, entry)| entry).collect();
        for entry in drained {
            let mut request_ids: HashSet<u64> = entry.waiters.keys().copied().collect();
            request_ids.extend(entry.receivers.keys().copied());
            for request_id in request_ids {
                self.continuations.remove(&request_id);
            }
            for (_, tx) in entry.waiters {
                let _ = tx.send(Err(ScriptError::new(
                    "external Xtra load cancelled with owner reset".to_owned(),
                )));
            }
        }
        self.completed_receivers.clear();
        self.continuations.clear();
    }

    pub(crate) fn take_load_waiter(
        &mut self,
        request: &ExternalXtraLoadRequest,
    ) -> Option<oneshot::Receiver<Result<(), ScriptError>>> {
        if request.state_id != self.state_id || !request.owner.is_arena_live() {
            return None;
        }
        // A completion may have moved this request's receiver out of the
        // pending name bucket while another same-name waiter remains live.
        // Resolve the opaque request id first so the older receiver is not
        // hidden by the newer pending entry.
        if let Some(retained) = self.completed_receivers.get(&request.request_id) {
            if retained.name != request.name || !retained.owner.same_identity(&request.owner) {
                return None;
            }
            return self
                .completed_receivers
                .remove(&request.request_id)
                .map(|retained| retained.receiver);
        }
        if let Some(entry) = self.pending_loads.get_mut(&request.name) {
            if !entry.owner.same_identity(&request.owner) {
                return None;
            }
            return entry.receivers.remove(&request.request_id);
        }
        None
    }

    /// Consume the suspended operation exactly once after a successful load.
    /// The caller must perform the final owner check immediately before
    /// executing the returned request.
    pub(crate) fn take_load_continuation(
        &mut self,
        request: &ExternalXtraLoadRequest,
    ) -> Option<ExternalXtraRequest> {
        let continuation = self.continuations.get(&request.request_id)?;
        if request.state_id != self.state_id
            || !request.owner.is_arena_live()
            || !continuation.owner.same_identity(&request.owner)
        {
            return None;
        }
        let continuation = self.continuations.remove(&request.request_id)?.continuation;
        let (xtra_name, args, operation) = match continuation {
            ExternalXtraContinuation::Static { xtra_name, handler, args } => (
                xtra_name,
                args,
                ExternalXtraOperation::Static { handler },
            ),
            ExternalXtraContinuation::Instance {
                xtra_name,
                instance_id,
                handler,
                args,
            } => (
                xtra_name,
                args,
                ExternalXtraOperation::Instance { instance_id, handler },
            ),
            ExternalXtraContinuation::Create { xtra_name, args } => (
                xtra_name,
                args,
                ExternalXtraOperation::Create,
            ),
        };
        Some(ExternalXtraRequest {
            owner: request.owner.clone(),
            owner_key: request.owner_key.clone(),
            xtra_name,
            operation,
            args,
        })
    }

    pub(crate) fn reset(&mut self) {
        self.cancel_pending_loads();
        self.continuations.clear();
        self.registered.clear();
        // A reset retires every outstanding browser capability.  Rotate the
        // state nonce before request IDs are reused so a late capability from
        // the previous generation cannot alias a new request with the same
        // numeric ID.
        self.state_id = NEXT_EXTERNAL_STATE_ID.fetch_add(1, Ordering::Relaxed);
        self.next_request_id = 1;
    }
}

/// Read the external registry from the owning player. The registry is not a
/// process-wide capability: a reset or replacement player starts with its
/// own set of loaded plugins.
pub(crate) fn is_registered_for_player(
    player: &DirPlayer,
    name: &str,
) -> bool {
    player.xtra_manager_state.external.is_registered(name)
}

pub(crate) fn registered_names_for_player(player: &DirPlayer) -> Vec<String> {
    player.xtra_manager_state.external.registered_names()
}

// ── Pending on-demand loads ──────────────────────────────────────────────
//
// When Lingo executes `new(xtra "X")` and X isn't registered, vm-rust asks
// JS to resolve the name through the registry and load the .wasm. The
// bytecode dispatch awaits a oneshot signal here; JS calls
// `complete_external_xtra_load(name, capability, success)` (exported in
// `lib.rs`) when the load finishes. The capability identifies the exact
// owner-local attempt, so a late same-name completion cannot satisfy a newer
// request.
//
// Multiple concurrent requests for the same name share one fetch — only
// the first requester triggers the JS callback; subsequent ones just
// append a receiver to the queue.

struct PendingLoad {
    owner: OwnerToken,
    /// One receiver per concurrent requester. When the load finishes
    /// `complete_load` drains the vec and signals each with the result.
    waiters: HashMap<u64, oneshot::Sender<Result<(), ScriptError>>>,
    receivers: HashMap<u64, oneshot::Receiver<Result<(), ScriptError>>>,
}

struct RetainedReceiver {
    owner: OwnerToken,
    name: String,
    receiver: oneshot::Receiver<Result<(), ScriptError>>,
}

struct PendingContinuation {
    owner: OwnerToken,
    continuation: ExternalXtraContinuation,
}

/// Ask the host to resolve `name` against its registry and load the
/// plugin. Returns `true` if the load succeeded (the caller can retry
/// `is_registered` and expect success); `false` if no registry entry
/// matched or the load itself failed. Resolves immediately if the name
/// is already registered, so callers can use this as a "ensure loaded"
/// check without a separate fast-path branch.
pub async fn request_xtra_load(
    state: &mut ExternalXtraState,
    owner: OwnerToken,
    owner_key: &str,
    name: &str,
) -> Result<(), ScriptError> {
    let key = name.to_lowercase();
    if state.is_registered(&key) {
        return Ok(());
    }
    let request = state.begin_load(owner, owner_key.to_owned(), &key)?;
    let receiver = state
        .take_load_waiter(&request)
        .ok_or_else(|| ScriptError::new("external Xtra load waiter was lost".to_owned()))?;
    if request.notify_host {
        // Fire-and-forget. JS will call complete_load with the result.
        js_bridge::onRequestXtraLoad(
            &request.name,
            &request.owner_key,
            &request.attempt_capability(),
        );
    }
    receiver
        .await
        .unwrap_or_else(|_| Err(ScriptError::new("external Xtra load waiter cancelled".to_owned())))
}

/// Prepare a known external static call while the owning player and symbol
/// table are borrowed. No JS bridge is touched here.
pub(crate) fn prepare_static_request(
    player: &DirPlayer,
    symbols: &SymbolTable,
    xtra_name: &str,
    handler_name: &str,
    args: &[DatumRef],
) -> Result<Option<ExternalXtraRequest>, ScriptError> {
    if !player.xtra_manager_state.external.is_registered(xtra_name) {
        return Ok(None);
    }
    Ok(Some(ExternalXtraRequest {
        owner: player.owner.clone(),
        owner_key: crate::player::owner_key_string(&player.owner),
        xtra_name: xtra_name.to_owned(),
        operation: ExternalXtraOperation::Static {
            handler: handler_name.to_ascii_lowercase(),
        },
        args: encode_args_from_player(player, args, symbols)?,
    }))
}

/// Prepare a bare static probe. Probing is also host work and therefore is
/// deferred with the eventual dispatch instead of running under the VM borrow.
pub(crate) fn prepare_static_probe(
    player: &DirPlayer,
    _symbols: &SymbolTable,
    fallback_name: Symbol,
    handler_name: &str,
    args: &[DatumRef],
) -> Result<Option<ExternalXtraRequest>, ScriptError> {
    let candidates = player.xtra_manager_state.external.registered_names();
    if candidates.is_empty() {
        return Ok(None);
    }
    Ok(Some(ExternalXtraRequest {
        owner: player.owner.clone(),
        owner_key: crate::player::owner_key_string(&player.owner),
        xtra_name: String::new(),
        operation: ExternalXtraOperation::ProbeStatic {
            handler: handler_name.to_ascii_lowercase(),
            candidates,
            raw_args: args.to_vec(),
            fallback_name,
            fallback_args: args.to_vec(),
        },
        // Probe ownership before converting arguments. An unrelated builtin
        // must retain its historical lazy argument errors when every plugin
        // declines the handler.
        args: Vec::new(),
    }))
}

/// Prepare an external instance call. Receiver identity validation remains in
/// the caller because it also owns the language-level foreign-reference
/// diagnostic; this function only serializes the already-selected operation.
pub(crate) fn prepare_instance_request(
    player: &DirPlayer,
    symbols: &SymbolTable,
    xtra_name: &str,
    instance_id: XtraInstanceId,
    handler_name: &str,
    args: &[DatumRef],
) -> Result<Option<ExternalXtraRequest>, ScriptError> {
    if !player.xtra_manager_state.external.is_registered(xtra_name) {
        return Ok(None);
    }
    Ok(Some(ExternalXtraRequest {
        owner: player.owner.clone(),
        owner_key: crate::player::owner_key_string(&player.owner),
        xtra_name: xtra_name.to_owned(),
        operation: ExternalXtraOperation::Instance {
            instance_id,
            handler: handler_name.to_ascii_lowercase(),
        },
        args: encode_args_from_player(player, args, symbols)?,
    }))
}

pub(crate) fn prepare_create_request(
    player: &DirPlayer,
    symbols: &SymbolTable,
    xtra_name: &str,
    args: &[DatumRef],
) -> Result<Option<ExternalXtraRequest>, ScriptError> {
    if !player.xtra_manager_state.external.is_registered(xtra_name) {
        return Ok(None);
    }
    Ok(Some(ExternalXtraRequest {
        owner: player.owner.clone(),
        owner_key: crate::player::owner_key_string(&player.owner),
        xtra_name: xtra_name.to_owned(),
        operation: ExternalXtraOperation::Create,
        args: encode_args_from_player(player, args, symbols)?,
    }))
}

/// Start an owner-bound on-demand load after the VM/session borrow has ended.
/// The opaque capability is retained by the caller so a synchronous JS
/// completion cannot lose its receiver.
pub(crate) fn start_load_request(request: &ExternalXtraLoadRequest) {
    js_bridge::onRequestXtraLoad(
        &request.name,
        &request.owner_key,
        &request.attempt_capability(),
    );
}

/// Prepare the synchronous xtra-to-xtra host operations. The plugin ABI
/// requires an immediate wire response, so this captures only owned request
/// data; the caller must release the player borrow before `execute_request`.
pub(crate) fn prepare_nested_host_request(
    player: &DirPlayer,
    op_id: u32,
    args: &[XDatum],
) -> Result<ExternalXtraRequest, String> {
        let op = HostOp::from_u32(op_id).ok_or_else(|| format!("unknown host op_id {}", op_id))?;
        let (xtra_name, operation, wire_args) = match op {
        HostOp::CreateXtraInstance => {
            let (Some(XDatum::String(name)), Some(XDatum::List(values))) = (args.first(), args.get(1)) else {
                return Err("create_xtra_instance: expected (String, List)".to_owned());
            };
            (name.clone(), ExternalXtraOperation::Create, wire::encode_args(values))
        }
        HostOp::CallXtraHandler => {
            let (Some(XDatum::String(name)), Some(XDatum::Int(id)), Some(XDatum::String(handler)), Some(XDatum::List(values))) =
                (args.first(), args.get(1), args.get(2), args.get(3)) else {
                return Err("call_xtra_handler: expected (String, Int, String, List)".to_owned());
            };
            if *id < 0 {
                return Err("call_xtra_handler: instance id must be non-negative".to_owned());
            }
            (
                name.clone(),
                ExternalXtraOperation::Instance {
                    instance_id: *id as XtraInstanceId,
                    handler: handler.to_ascii_lowercase(),
                },
                wire::encode_args(values),
            )
        }
        HostOp::DestroyXtraInstance => {
            let (Some(XDatum::String(name)), Some(XDatum::Int(id))) = (args.first(), args.get(1)) else {
                return Err("destroy_xtra_instance: expected (String, Int)".to_owned());
            };
            if *id < 0 {
                return Err("destroy_xtra_instance: instance id must be non-negative".to_owned());
            }
            (
                name.clone(),
                ExternalXtraOperation::Destroy { instance_id: *id as XtraInstanceId },
                wire::encode_args(&[]),
            )
        }
        _ => return Err(format!("host op {} is not an inter-Xtra operation", op_id)),
    };
    if !player.xtra_manager_state.external.is_registered(&xtra_name) {
        return Err(format!("external Xtra '{}' is not loaded", xtra_name));
    }
    Ok(ExternalXtraRequest {
        owner: player.owner.clone(),
        owner_key: crate::player::owner_key_string(&player.owner),
        xtra_name,
        operation,
        args: wire_args,
    })
}


/// Prepare a continuation-backed load request. The caller must invoke this
/// while holding the owning player borrow; no host code runs here.
pub(crate) fn prepare_load_request(
    player: &mut DirPlayer,
    continuation: ExternalXtraContinuation,
) -> Result<ExternalXtraLoadRequest, ScriptError> {
    let name = match &continuation {
        ExternalXtraContinuation::Static { xtra_name, .. }
        | ExternalXtraContinuation::Instance { xtra_name, .. }
        | ExternalXtraContinuation::Create { xtra_name, .. } => xtra_name,
    };
    let owner = player.owner.clone();
    let owner_key = crate::player::owner_key_string(&owner);
    let request = player
        .xtra_manager_state
        .external
        .begin_load(owner, owner_key, name)?;
    player
        .xtra_manager_state
        .external
        .attach_continuation(&request, continuation)?;
    Ok(request)
}

/// Execute a prepared call after the session borrow has ended. The optional
/// result is `None` only for a static probe with no matching plugin.
pub(crate) fn execute_request(
    request: &ExternalXtraRequest,
) -> Result<Option<ExternalXtraResponse>, ScriptError> {
    ensure_request_owner_live(request)?;
    match &request.operation {
        ExternalXtraOperation::ProbeStatic { handler, candidates, .. } => {
            for candidate in candidates {
                ensure_request_owner_live(request)?;
                if js_bridge::externalXtraHasStaticHandler(
                    candidate, handler, &request.owner_key,
                ) == 0 {
                    ensure_request_owner_live(request)?;
                    continue;
                }
                ensure_request_owner_live(request)?;
                ensure_request_owner_live(request)?;
                return Ok(Some(ExternalXtraResponse {
                    xtra_name: candidate.clone(),
                    // The probe only establishes ownership. The command
                    // executor serializes raw args under a short owner borrow
                    // and performs the actual dispatch in a second request.
                    bytes: Vec::new(),
                }));
            }
            Ok(None)
        }
        ExternalXtraOperation::Static { handler } => {
            ensure_request_owner_live(request)?;
            let result = js_bridge::dispatchExternalXtraStaticHandler(
                &request.xtra_name, handler, &request.args, &request.owner_key,
            );
            ensure_request_owner_live(request)?;
            let result = result.ok_or_else(|| ScriptError::new(format!(
                "External xtra '{}' static dispatch returned None",
                request.xtra_name
            )))?;
            Ok(Some(ExternalXtraResponse {
                xtra_name: request.xtra_name.clone(),
                bytes: result,
            }))
        }
        ExternalXtraOperation::Instance { instance_id, handler } => {
            ensure_request_owner_live(request)?;
            let result = js_bridge::dispatchExternalXtraInstanceHandler(
                &request.xtra_name, *instance_id as u32, handler,
                &request.args, &request.owner_key,
            );
            ensure_request_owner_live(request)?;
            let result = result.ok_or_else(|| ScriptError::new(format!(
                "External xtra '{}' instance dispatch returned None",
                request.xtra_name
            )))?;
            Ok(Some(ExternalXtraResponse {
                xtra_name: request.xtra_name.clone(),
                bytes: result,
            }))
        }
        ExternalXtraOperation::Create => {
            ensure_request_owner_live(request)?;
            let result = js_bridge::createExternalXtraInstance(
                &request.xtra_name, &request.args, &request.owner_key,
            );
            ensure_request_owner_live(request)?;
            let result = result.ok_or_else(|| ScriptError::new(format!(
                "External xtra '{}' create dispatch returned None",
                request.xtra_name
            )))?;
            Ok(Some(ExternalXtraResponse {
                xtra_name: request.xtra_name.clone(),
                bytes: result,
            }))
        }
        ExternalXtraOperation::Destroy { instance_id } => {
            ensure_request_owner_live(request)?;
            js_bridge::destroyExternalXtraInstance(
                &request.xtra_name, *instance_id as u32, &request.owner_key,
            );
            ensure_request_owner_live(request)?;
            Ok(Some(ExternalXtraResponse {
                xtra_name: request.xtra_name.clone(),
                bytes: Vec::new(),
            }))
        }
    }
}

fn ensure_request_owner_live(request: &ExternalXtraRequest) -> Result<(), ScriptError> {
    if !request.owner.is_arena_live() {
        return Err(ScriptError::new_code(
            crate::player::ScriptErrorCode::Abort,
            "external Xtra owner was retired during dispatch".to_owned(),
        ));
    }
    Ok(())
}

/// Apply an external response to the exact owner that prepared it. No
/// ambient player lookup is permitted here.
pub(crate) fn finish_request(
    player: &mut DirPlayer,
    symbols: &mut SymbolTable,
    request: &ExternalXtraRequest,
    response: &ExternalXtraResponse,
) -> Result<DatumRef, ScriptError> {
    if !request.owner.is_arena_live() || !request.owner.same_identity(&player.owner) {
        return Err(ScriptError::new_code(
            crate::player::ScriptErrorCode::InvalidReference,
            "stale or foreign external Xtra completion".to_owned(),
        ));
    }
    if matches!(&request.operation, ExternalXtraOperation::Destroy { .. }) {
        return Ok(DatumRef::Void);
    }
    let handler = match &request.operation {
        ExternalXtraOperation::ProbeStatic { handler, .. }
        | ExternalXtraOperation::Static { handler }
        | ExternalXtraOperation::Instance { handler, .. } => handler.as_str(),
        ExternalXtraOperation::Create => "new",
        ExternalXtraOperation::Destroy { .. } => "destroy",
    };
    decode_return_to_datum_ref(
        &response.bytes, &response.xtra_name, handler, player, symbols,
    )
}

// ── JS bridge (declared in `dirplayer-js-api`) ───────────────────────────

#[cfg(target_arch = "wasm32")]
mod js_bridge {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(module = "dirplayer-js-api")]
    extern "C" {
        /// Calls the plugin's `__xtra_call_static_handler`. Returns the
        /// raw postcard `WireFrame::Return` (or `Error`) bytes, or `None`
        /// if the xtra isn't loaded.
        pub fn dispatchExternalXtraStaticHandler(
            xtra_name: &str,
            handler: &str,
            args: &[u8],
            owner_key: &str,
        ) -> Option<Vec<u8>>;

        /// Calls the plugin's `__xtra_call_handler`. Returns the postcard
        /// frame bytes or `None` if the xtra isn't loaded.
        pub fn dispatchExternalXtraInstanceHandler(
            xtra_name: &str,
            instance_id: u32,
            handler: &str,
            args: &[u8],
            owner_key: &str,
        ) -> Option<Vec<u8>>;

        /// Calls the plugin's `__xtra_create_instance`. Returns the
        /// postcard frame bytes (whose decoded `Datum::Int` carries the
        /// instance id), or `None` if the xtra isn't loaded.
        pub fn createExternalXtraInstance(
            xtra_name: &str,
            args: &[u8],
            owner_key: &str,
        ) -> Option<Vec<u8>>;

        /// Calls the plugin's `__xtra_destroy_instance`. No return value.
        pub fn destroyExternalXtraInstance(
            xtra_name: &str,
            instance_id: u32,
            owner_key: &str,
        );

        /// Returns `1` if the plugin reports the handler as a static
        /// handler. `0` otherwise. Mirrors `__xtra_has_static_handler`.
        pub fn externalXtraHasStaticHandler(
            xtra_name: &str,
            handler: &str,
            owner_key: &str,
        ) -> u32;

        /// Fetch a plugin .wasm from `url`, instantiate it, and register
        /// the xtra. Resolves with the registered xtra name. Used by the
        /// test harness's `player.load_external_xtra(...)` helper —
        /// production code paths normally go through the JS-side
        /// `loadExternalXtras` config loader (localStorage in dev,
        /// init-script in polyfill, etc.).
        #[wasm_bindgen(catch)]
        pub async fn loadExternalXtra(url: &str) -> Result<JsValue, JsValue>;

        /// Tell JS to resolve `name` against the registry and load the
        /// plugin asynchronously. JS calls back via the wasm-bindgen
        /// export `complete_external_xtra_load(name, capability, success)` (see
        /// `lib.rs`) when the load finishes — there is no return value
        /// here; this function is fire-and-forget. Fired by
        /// `request_xtra_load` when an unknown xtra is hit by Lingo.
        pub fn onRequestXtraLoad(name: &str, owner_key: &str, capability: &str);

        /// Drop host-side plugin slots and pending load bookkeeping for one
        /// retired browser owner generation.
        pub fn disposeExternalXtraHost(owner_key: &str);
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod js_bridge {
    // Native-target stubs for `cargo check` / unit tests. The wasm32
    // build is the only target that actually loads plugins.
    pub fn dispatchExternalXtraStaticHandler(
        _: &str, _: &str, _: &[u8], _: &str,
    ) -> Option<Vec<u8>> { None }
    pub fn dispatchExternalXtraInstanceHandler(
        _: &str, _: u32, _: &str, _: &[u8], _: &str,
    ) -> Option<Vec<u8>> { None }
    pub fn createExternalXtraInstance(_: &str, _: &[u8], _: &str) -> Option<Vec<u8>> { None }
    pub fn destroyExternalXtraInstance(_: &str, _: u32, _: &str) {}
    pub fn externalXtraHasStaticHandler(_: &str, _: &str, _: &str) -> u32 { 0 }
    pub fn onRequestXtraLoad(_: &str, _: &str, _: &str) {}
    pub fn disposeExternalXtraHost(_: &str) {}
}

pub(crate) fn dispose_external_host(owner_key: &str) {
    js_bridge::disposeExternalXtraHost(owner_key);
}

/// Destroy an external xtra instance. Idempotent.
pub fn destroy_instance(
    state: &ExternalXtraState,
    xtra_name: &str,
    instance_id: XtraInstanceId,
    owner_key: &str,
) {
    if !state.is_registered(xtra_name) {
        return;
    }
    js_bridge::destroyExternalXtraInstance(xtra_name, instance_id as u32, owner_key);
}

// ── Test-harness plugin loader ──────────────────────────────────────────

/// Fetch a plugin .wasm from `url`, instantiate it, and register the
/// xtra. Returns the registered xtra name on success. Used by e2e tests
/// (`BrowserTestPlayer::load_external_xtra`). Production code paths
/// normally load plugins via the JS-side `loadExternalXtras` configured
/// per-host (localStorage in dev, init-script in polyfill, etc.).
#[cfg(target_arch = "wasm32")]
pub async fn load_for_test(url: &str) -> Result<String, String> {
    match js_bridge::loadExternalXtra(url).await {
        Ok(name_val) => name_val
            .as_string()
            .ok_or_else(|| format!("loadExternalXtra({}): returned non-string name", url)),
        Err(e) => Err(format!(
            "loadExternalXtra({}): {}",
            url,
            e.as_string().unwrap_or_else(|| String::from("(opaque JsValue error)"))
        )),
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn load_for_test(_url: &str) -> Result<String, String> {
    Err(String::from("load_for_test is only available on wasm32"))
}

// ── dx_host_call dispatcher (called by JS for every plugin host call) ───

/// Single dispatch entry point for every `dx_host_call` from any plugin.
/// JS reads the args from plugin memory, passes them here, and writes the
/// resulting bytes back into plugin memory.
///
/// Returns the postcard-encoded result frame (`WireFrame::Return` or
/// `WireFrame::Error`) — or an empty `Vec` for the "void" sentinel which
/// lets fire-and-forget ops (like `log`) skip a postcard round-trip.
pub fn host_call_dispatch(
    player: &mut DirPlayer,
    symbols: &mut SymbolTable,
    op_id: u32,
    args_bytes: &[u8],
) -> Vec<u8> {
    let op = match HostOp::from_u32(op_id) {
        Some(o) => o,
        None => {
            return wire::encode_error(&format!(
                "unknown host op_id {}",
                op_id
            ));
        }
    };
    let args = match wire::decode_args(args_bytes) {
        Ok(a) => a,
        Err(e) => return wire::encode_error(&format!("bad args: {}", e)),
    };
    match op {
        HostOp::Log => {
            if let Some(XDatum::String(msg)) = args.first() {
                #[cfg(target_arch = "wasm32")]
                web_sys::console::log_1(&format!("[xtra] {}", msg).into());
                #[cfg(not(target_arch = "wasm32"))]
                log::info!("[xtra] {}", msg);
            }
            Vec::new() // void sentinel
        }
        HostOp::RandomFill => {
            let len = match args.first() {
                Some(XDatum::Int(n)) => *n as usize,
                _ => return wire::encode_error("random_fill: expected Int len"),
            };
            // Cap at a reasonable size to avoid DoS via a malicious plugin
            // asking for gigabytes. BobbaXtra's largest call is 32 bytes
            // (machine-id seed); DH ephemeral keys are similar.
            if len > 1 << 20 {
                return wire::encode_error("random_fill: requested length too large");
            }
            let mut buf = vec![0u8; len];
            #[cfg(target_arch = "wasm32")]
            {
                match web_sys::window().and_then(|w| w.crypto().ok()) {
                    Some(crypto) => {
                        if crypto.get_random_values_with_u8_array(&mut buf).is_err() {
                            return wire::encode_error("random_fill: getRandomValues failed");
                        }
                    }
                    None => return wire::encode_error("random_fill: no crypto in window"),
                }
            }
            wire::encode_return(&XDatum::ByteArray(buf))
        }
        HostOp::StorageGet => {
            let key = match args.first() {
                Some(XDatum::String(s)) => s.as_str(),
                _ => {
                    return wire::encode_error("storage_get: expected String key");
                }
            };
            #[cfg(target_arch = "wasm32")]
            {
                if let Some(window) = web_sys::window() {
                    if let Ok(Some(storage)) = window.local_storage() {
                        if let Ok(Some(val)) = storage.get_item(key) {
                            return wire::encode_return(&XDatum::String(val));
                        }
                    }
                }
            }
            let _ = key;
            wire::encode_return(&XDatum::Void)
        }
        HostOp::StorageSet => {
            let (key, val) = match (args.first(), args.get(1)) {
                (Some(XDatum::String(k)), Some(XDatum::String(v))) => (k.as_str(), v.as_str()),
                _ => {
                    return wire::encode_error("storage_set: expected (key, val)");
                }
            };
            #[cfg(target_arch = "wasm32")]
            {
                if let Some(window) = web_sys::window() {
                    if let Ok(Some(storage)) = window.local_storage() {
                        match storage.set_item(key, val) {
                            Ok(()) => return Vec::new(),
                            Err(_) => return wire::encode_error("localStorage.setItem failed"),
                        }
                    }
                }
            }
            let _ = (key, val);
            wire::encode_error("storage_set: no localStorage available")
        }
        HostOp::CreateXtraInstance
        | HostOp::CallXtraHandler
        | HostOp::DestroyXtraInstance => {
            // This lower-level entrypoint is called while its caller owns a
            // VM borrow.  Executing JavaScript here would permit nested Xtra
            // re-entry with that borrow still live.  The owner-bound browser
            // handle uses `prepare_nested_host_request` followed by
            // `execute_request` after releasing the borrow.
            let _ = (player, symbols, args);
            wire::encode_error("external Xtra host calls require the owner-bound executor")
        }

        // ── 3D scene rendering ────────────────────────────────────────────
        // These arrive mid-Lingo-execution (no GL context in scope), so they
        // only mutate the CPU-side scene store; the webgl2 `XtraSceneRenderer`
        // uploads and composites during the normal draw pass. The canonical
        // prepare/execute/finish route invokes the plugin after releasing the
        // player borrow, so a nested host request can be scheduled without
        // retaining a VM borrow across the host boundary.
        HostOp::Scene3dCreate => {
            let tag = match args.first() {
                Some(XDatum::String(s)) => s.clone(),
                _ => return wire::encode_error("scene3d_create: expected String tag"),
            };
            let id = player.scene3d_store.create(&tag);
            wire::encode_return(&XDatum::Int(id))
        }
        HostOp::Scene3dUploadMesh => {
            let (scene_id, mesh_id, bytes) = match (args.first(), args.get(1), args.get(2)) {
                (Some(XDatum::Int(s)), Some(XDatum::Int(m)), Some(XDatum::ByteArray(b))) => {
                    (*s, *m as u32, b)
                }
                _ => return wire::encode_error("scene3d_upload_mesh: expected (Int, Int, ByteArray)"),
            };
            match SceneMeshData::from_bytes(bytes) {
                Ok(data) => {
                    player.scene3d_store.upload_mesh(scene_id, mesh_id, data);
                    Vec::new()
                }
                Err(e) => wire::encode_error(&format!("scene3d_upload_mesh: bad MeshData: {:?}", e)),
            }
        }
        HostOp::Scene3dDropMesh => {
            let (scene_id, mesh_id) = match (args.first(), args.get(1)) {
                (Some(XDatum::Int(s)), Some(XDatum::Int(m))) => (*s, *m as u32),
                _ => return wire::encode_error("scene3d_drop_mesh: expected (Int, Int)"),
            };
            player.scene3d_store.drop_mesh(scene_id, mesh_id);
            Vec::new()
        }
        HostOp::Scene3dUploadTexture => {
            let (scene_id, name, w, h, rgba) = match (
                args.first(), args.get(1), args.get(2), args.get(3), args.get(4),
            ) {
                (
                    Some(XDatum::Int(s)), Some(XDatum::String(n)),
                    Some(XDatum::Int(w)), Some(XDatum::Int(h)), Some(XDatum::ByteArray(b)),
                ) => (*s, n.clone(), *w as u32, *h as u32, b.clone()),
                _ => {
                    return wire::encode_error(
                        "scene3d_upload_texture: expected (Int, String, Int, Int, ByteArray)",
                    );
                }
            };
            player.scene3d_store.upload_texture(scene_id, &name, w, h, rgba);
            Vec::new()
        }
        HostOp::Scene3dSubmitFrame => {
            let (scene_id, bytes) = match (args.first(), args.get(1)) {
                (Some(XDatum::Int(s)), Some(XDatum::ByteArray(b))) => (*s, b),
                _ => return wire::encode_error("scene3d_submit_frame: expected (Int, ByteArray)"),
            };
            match SceneFrameData::from_bytes(bytes) {
                Ok(frame) => {
                    let movie_frame = player.movie.current_frame as i32;
                    player.scene3d_store.submit_frame(scene_id, frame, movie_frame);
                    Vec::new()
                }
                Err(e) => wire::encode_error(&format!("scene3d_submit_frame: bad FrameData: {:?}", e)),
            }
        }
        HostOp::Scene3dDestroy => {
            let scene_id = match args.first() {
                Some(XDatum::Int(s)) => *s,
                _ => return wire::encode_error("scene3d_destroy: expected Int scene_id"),
            };
            player.scene3d_store.destroy(scene_id);
            Vec::new()
        }

        // ── Host state a compute-only plugin reads ────────────────────────
        HostOp::CastMemberBytes => {
            use crate::player::cast_member::CastMemberType;
            let (name, kind) = match (args.first(), args.get(1)) {
                (Some(XDatum::String(n)), Some(XDatum::String(k))) => (n.clone(), k.to_lowercase()),
                _ => return wire::encode_error("cast_member_bytes: expected (String, String)"),
            };
            let bytes = player
                .movie
                .cast_manager
                .find_member_ref_by_name(&name)
                .and_then(|mref| player.movie.cast_manager.find_member_by_ref(&mref))
                .and_then(|member| match (kind.as_str(), &member.member_type) {
                    ("groove3gm", CastMemberType::Groove3gm(m)) => Some(m.data.clone()),
                    _ => None,
                });
            match bytes {
                Some(b) => wire::encode_return(&XDatum::ByteArray(b)),
                None => wire::encode_return(&XDatum::Void),
            }
        }
        HostOp::MemberSize => {
            let name = match args.first() {
                Some(XDatum::String(s)) => s.clone(),
                _ => return wire::encode_error("member_size: expected String name"),
            };
            use crate::player::cast_member::CastMemberType;
            let size = player
                .movie
                .cast_manager
                .find_member_ref_by_name(&name)
                .and_then(|mref| player.movie.cast_manager.find_member_by_ref(&mref))
                .and_then(|member| match &member.member_type {
                    CastMemberType::Bitmap(b) => player.bitmap_manager.get_bitmap(b.image_ref),
                    _ => None,
                })
                .map(|bitmap| (bitmap.width as i32, bitmap.height as i32));
            match size {
                Some((w, h)) => {
                    wire::encode_return(&XDatum::List(vec![XDatum::Int(w), XDatum::Int(h)]))
                }
                None => wire::encode_return(&XDatum::Void),
            }
        }
        HostOp::StageInfo => {
            let (w, h, frame) = (player.movie.rect.width(), player.movie.rect.height(), player.movie.current_frame as i32);
            wire::encode_return(&XDatum::List(vec![
                XDatum::Int(w),
                XDatum::Int(h),
                XDatum::Int(frame),
            ]))
        }
        HostOp::MouseLoc => {
            let (x, y) = player.mouse_loc;
            wire::encode_return(&XDatum::Point(x as f64, y as f64))
        }
        HostOp::KeyDown => {
            let key = match args.first() {
                Some(XDatum::String(s)) => s.clone(),
                _ => return wire::encode_error("key_down: expected String key"),
            };
            let down = player.keyboard_manager.is_key_down(&key);
            wire::encode_return(&XDatum::Bool(down))
        }
        HostOp::SetLingoGlobal => {
            let (name, value) = match (args.first(), args.get(1)) {
                (Some(XDatum::String(n)), Some(v)) => (n.clone(), v.clone()),
                _ => return wire::encode_error("set_lingo_global: expected (String, value)"),
            };
            let value_ref = {
                let host = xdatum_to_host_datum(&value, player, symbols);
                player.alloc_datum(host)
            };
            player.globals.insert(symbols.intern(&name), value_ref);
            Vec::new()
        }
    }
}

// ── Helpers: Datum <-> DatumRef conversion ───────────────────────────────

/// Encode a `&Vec<DatumRef>` as a postcard `WireFrame::Args` payload.
/// Returns `None` on serialization failure — the caller surfaces this as
/// a None bubble to the dispatch arm.
pub(crate) fn encode_args_from_player(
    player: &DirPlayer,
    args: &[DatumRef],
    symbols: &SymbolTable,
) -> Result<Vec<u8>, ScriptError> {
    let xs: Result<Vec<XDatum>, ScriptError> = args
        .iter()
        .map(|r| host_datum_to_xdatum(&player.get_datum(r).clone(), player, symbols))
        .collect();
    xs.map(|xs| wire::encode_args(&xs))
}

/// Convert a host-side `Datum` to an SDK-side `XDatum`. The mapping is
/// 1:1 for variants present in both; host-only variants (Color, CastRef,
/// BitmapRef, etc.) become `XDatum::Void` for now — a future revision of
/// the WIT contract can extend the variant set.
///
/// Director represents booleans as `Datum::Int(0)` / `Datum::Int(1)`
/// (there is no separate Bool variant on the host), so we forward Ints
/// as-is. Plugins that want bool semantics can compare to `Int(0)`.
fn host_datum_to_xdatum(d: &Datum, player: &DirPlayer, symbols: &SymbolTable) -> Result<XDatum, ScriptError> {
    let invalid_symbol = || ScriptError::new_code(
        crate::player::ScriptErrorCode::InvalidReference,
        "foreign or stale symbol in external Xtra argument".to_owned(),
    );
    match d {
        Datum::Void => Ok(XDatum::Void),
        Datum::Int(i) => Ok(XDatum::Int(*i)),
        Datum::Float(f) => Ok(XDatum::Float(*f)),
        Datum::String(s) => Ok(XDatum::String(s.clone())),
        Datum::Symbol(s) => Ok(XDatum::Symbol(symbols.display(s).map_err(|_| invalid_symbol())?.to_owned())),
        // Container variants recurse, resolving each child DatumRef through
        // the player's datum arena. Groove passes/returns lists, prop-lists,
        // points and rects, so these must survive the boundary intact.
        Datum::List(_, items, _) => Ok(XDatum::List(
            items
                .iter()
                .map(|r| host_datum_to_xdatum(player.get_datum(r), player, symbols))
                .collect::<Result<_, _>>()?,
        )),
        Datum::PropList(pairs, _) => Ok(XDatum::PropList(
            pairs
                .iter()
                .map(|(k, v)| {
                    Ok::<(XDatum, XDatum), ScriptError>((
                        host_datum_to_xdatum(player.get_datum(k), player, symbols)?,
                        host_datum_to_xdatum(player.get_datum(v), player, symbols)?,
                    ))
                })
                .collect::<Result<_, _>>()?,
        )),
        Datum::Point([x, y], _) => Ok(XDatum::Point(*x, *y)),
        Datum::Rect([l, t, r, b], _) => Ok(XDatum::Rect(*l, *t, *r, *b)),
        _ => Ok(XDatum::Void),
    }
}

/// Convert an SDK-side `XDatum` back to a host `Datum` and allocate it
/// into the player's datum manager, returning a fresh `DatumRef`.
fn xdatum_to_host_datum_ref(
    d: &XDatum,
    player: &mut DirPlayer,
    symbols: &mut SymbolTable,
) -> DatumRef {
    let host = xdatum_to_host_datum(d, player, symbols);
    player.alloc_datum(host)
}

fn xdatum_to_host_datum(d: &XDatum, player: &mut DirPlayer, symbols: &mut SymbolTable) -> Datum {
    match d {
        XDatum::Void => Datum::Void,
        XDatum::Int(i) => Datum::Int(*i),
        XDatum::Float(f) => Datum::Float(*f),
        XDatum::String(s) => Datum::String(s.clone()),
        XDatum::Symbol(s) => Datum::Symbol(symbols.intern(s)),
        // Container variants recurse: each child is converted and allocated
        // into the player's datum arena first, then the parent references the
        // fresh DatumRefs. Mirrors `host_datum_to_xdatum` in the other
        // direction so Groove's list/prop-list/point/rect returns survive.
        XDatum::List(items) => {
            let refs: VecDeque<DatumRef> = items
                .iter()
                .map(|it| {
                    let child = xdatum_to_host_datum(it, player, symbols);
                    player.alloc_datum(child)
                })
                .collect();
            Datum::List(DatumType::List, refs, false)
        }
        XDatum::PropList(pairs) => {
            let entries: VecDeque<(DatumRef, DatumRef)> = pairs
                .iter()
                .map(|(k, v)| {
                    let kd = xdatum_to_host_datum(k, player, symbols);
                    let kr = player.alloc_datum(kd);
                    let vd = xdatum_to_host_datum(v, player, symbols);
                    let vr = player.alloc_datum(vd);
                    (kr, vr)
                })
                .collect();
            Datum::PropList(entries, false)
        }
        XDatum::Point(x, y) => Datum::Point([*x, *y], 0),
        XDatum::Rect(l, t, r, b) => Datum::Rect([*l, *t, *r, *b], 0),
        // Director booleans are Int(0)/Int(1). Use the helper so a future
        // change to the bool representation only touches one place.
        XDatum::Bool(b) => datum_bool(*b),
        // Byte payloads round-trip into Director as a Latin-1 string
        // (byte b → char b). This matches how multiuser/fileio surface
        // binary data to Lingo: the high-bit-set chars stay distinct,
        // and `string_value()` on the host still yields the original
        // bytes via `c as u8`. Plugins that round-trip raw bytes hand
        // them back to other plugins through `host_env::call_xtra_handler`
        // using ByteArray directly.
        XDatum::ByteArray(b) => {
            Datum::String(b.iter().map(|&byte| byte as char).collect())
        }
        _ => Datum::Void,
    }
}

/// Decode a wire-level return frame into a host `Result<DatumRef, ScriptError>`.
fn decode_return_to_datum_ref(
    bytes: &[u8],
    xtra_name: &str,
    handler_name: &str,
    player: &mut DirPlayer,
    symbols: &mut SymbolTable,
) -> Result<DatumRef, ScriptError> {
    match wire::decode_return(bytes) {
        Ok(d) => Ok(xdatum_to_host_datum_ref(&d, player, symbols)),
        Err(e) => Err(ScriptError::new(format!(
            "{}.{}: {}",
            xtra_name, handler_name, e
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_probe_identity_survives_finish_boundary() {
        let owner = OwnerToken::transitional();
        let (command_tx, _) = async_std::channel::unbounded();
        let mut player = DirPlayer::new_with_owner(command_tx, owner.clone());
        let mut symbols = SymbolTable::new();
        let request = ExternalXtraRequest {
            owner,
            owner_key: "test-owner".to_owned(),
            xtra_name: String::new(),
            operation: ExternalXtraOperation::ProbeStatic {
                handler: "probe".to_owned(),
                candidates: vec!["first".to_owned(), "selected".to_owned()],
                raw_args: Vec::new(),
                fallback_name: Symbol::empty(),
                fallback_args: Vec::new(),
            },
            args: Vec::new(),
        };
        let response = ExternalXtraResponse {
            xtra_name: "selected".to_owned(),
            bytes: vec![0xff, 0xff],
        };
        let error = match finish_request(&mut player, &mut symbols, &request, &response) {
            Ok(_) => panic!("malformed selected response unexpectedly decoded"),
            Err(error) => error,
        };
        assert!(error.message.contains("selected.probe"));
    }

    #[test]
    fn retired_probe_owner_is_rejected_before_host_dispatch() {
        let owner = OwnerToken::transitional();
        let request = ExternalXtraRequest {
            owner: owner.clone(),
            owner_key: "test-owner".to_owned(),
            xtra_name: "selected".to_owned(),
            operation: ExternalXtraOperation::Static {
                handler: "probe".to_owned(),
            },
            args: Vec::new(),
        };
        owner.begin_reset();
        assert!(ensure_request_owner_live(&request).is_err());
    }

    #[test]
    fn successful_load_retains_original_operation_until_taken() {
        let owner = OwnerToken::transitional();
        let mut state = ExternalXtraState::default();
        let load = state
            .begin_load(owner, "test-owner".to_owned(), "ExampleXtra")
            .unwrap();
        state
            .attach_continuation(
                &load,
                ExternalXtraContinuation::Create {
                    xtra_name: "ExampleXtra".to_owned(),
                    args: vec![1, 2, 3],
                },
            )
            .unwrap();
        state.complete_load(
            &load.owner,
            load.state_id,
            load.request_id,
            "ExampleXtra",
            true,
        );
        let request = state.take_load_continuation(&load).expect("continuation");
        assert_eq!(request.xtra_name, "ExampleXtra");
        assert_eq!(request.args, vec![1, 2, 3]);
        assert!(matches!(request.operation, ExternalXtraOperation::Create));
        assert!(state.take_load_continuation(&load).is_none());
    }

    #[test]
    fn duplicate_continuation_attachment_is_rejected_without_replacing_original() {
        let owner = OwnerToken::transitional();
        let mut state = ExternalXtraState::default();
        let load = state
            .begin_load(owner, "test-owner".to_owned(), "ExampleXtra")
            .unwrap();
        state
            .attach_continuation(
                &load,
                ExternalXtraContinuation::Create {
                    xtra_name: "ExampleXtra".to_owned(),
                    args: vec![1],
                },
            )
            .unwrap();
        assert!(state
            .attach_continuation(
                &load,
                ExternalXtraContinuation::Create {
                    xtra_name: "OtherXtra".to_owned(),
                    args: vec![2],
                },
            )
            .is_err());
        state.complete_load(
            &load.owner,
            load.state_id,
            load.request_id,
            "ExampleXtra",
            true,
        );
        let request = state.take_load_continuation(&load).expect("original continuation");
        assert_eq!(request.xtra_name, "ExampleXtra");
        assert_eq!(request.args, vec![1]);
    }

    #[test]
    fn stale_same_name_completion_cannot_satisfy_newer_attempt() {
        let owner = OwnerToken::transitional();
        let mut state = ExternalXtraState::default();
        let first = state
            .begin_load(owner.clone(), "test-owner".to_owned(), "ExampleXtra")
            .unwrap();
        state.complete_load(
            &owner,
            first.state_id,
            first.request_id,
            "ExampleXtra",
            false,
        );
        let mut first_receiver = state.take_load_waiter(&first).expect("first receiver");
        let second = state
            .begin_load(owner, "test-owner".to_owned(), "ExampleXtra")
            .unwrap();
        let mut second_receiver = state.take_load_waiter(&second).expect("second receiver");
        state.complete_load(
            &second.owner,
            second.state_id,
            first.request_id,
            "ExampleXtra",
            true,
        );
        assert!(matches!(first_receiver.try_recv(), Ok(Some(Err(_)))));
        assert!(matches!(second_receiver.try_recv(), Ok(None)));
        state.complete_load(
            &second.owner,
            second.state_id,
            second.request_id,
            "ExampleXtra",
            true,
        );
        // The stale completion did not satisfy the newer request; only the
        // exact request capability can deliver its success.
        assert!(matches!(second_receiver.try_recv(), Ok(Some(Ok(())))));
    }

    #[test]
    fn load_capabilities_reject_foreign_state_with_same_request_id() {
        let owner_a = OwnerToken::transitional();
        let owner_b = OwnerToken::transitional();
        let mut state_a = ExternalXtraState::default();
        let mut state_b = ExternalXtraState::default();
        let request_a = state_a
            .begin_load(owner_a, "owner-a".to_owned(), "ExampleXtra")
            .unwrap();
        let request_b = state_b
            .begin_load(owner_b, "owner-b".to_owned(), "ExampleXtra")
            .unwrap();
        assert_eq!(request_a.request_id, request_b.request_id);
        state_b
            .attach_continuation(
                &request_b,
                ExternalXtraContinuation::Create {
                    xtra_name: "ExampleXtra".to_owned(),
                    args: Vec::new(),
                },
            )
            .unwrap();
        assert!(state_b.take_load_waiter(&request_a).is_none());
        assert!(state_b.take_load_continuation(&request_a).is_none());
        assert!(state_b.take_load_continuation(&request_b).is_some());
    }

    #[test]
    fn load_capabilities_reject_same_owner_from_foreign_state() {
        let owner = OwnerToken::transitional();
        let mut state_a = ExternalXtraState::default();
        let mut state_b = ExternalXtraState::default();
        let request_a = state_a
            .begin_load(owner.clone(), "owner".to_owned(), "ExampleXtra")
            .unwrap();
        let request_b = state_b
            .begin_load(owner, "owner".to_owned(), "ExampleXtra")
            .unwrap();
        assert_eq!(request_a.request_id, request_b.request_id);
        assert!(state_b.take_load_waiter(&request_a).is_none());
        assert!(state_b.take_load_continuation(&request_a).is_none());
    }

    #[test]
    fn synchronous_completion_retains_receiver_until_executor_takes_it() {
        let owner = OwnerToken::transitional();
        let mut state = ExternalXtraState::default();
        let request = state
            .begin_load(owner, "test-owner".to_owned(), "ExampleXtra")
            .unwrap();
        state.complete_load(
            &request.owner,
            request.state_id,
            request.request_id,
            "ExampleXtra",
            true,
        );
        let second = state
            .begin_load(
                request.owner.clone(),
                "test-owner".to_owned(),
                "ExampleXtra",
            )
            .unwrap();
        assert_ne!(request.request_id, second.request_id);
        let mut receiver = state.take_load_waiter(&request).expect("completed receiver");
        assert!(matches!(receiver.try_recv(), Ok(Some(Ok(())))));
    }

    #[test]
    fn foreign_begin_does_not_drop_completed_receiver() {
        let owner = OwnerToken::transitional();
        let foreign_owner = OwnerToken::transitional();
        let mut state = ExternalXtraState::default();
        let request = state
            .begin_load(owner, "test-owner".to_owned(), "ExampleXtra")
            .unwrap();
        state.complete_load(
            &request.owner,
            request.state_id,
            request.request_id,
            "ExampleXtra",
            true,
        );
        assert!(state
            .begin_load(foreign_owner, "foreign-owner".to_owned(), "ExampleXtra")
            .is_err());
        assert!(state.take_load_waiter(&request).is_some());
    }

    #[test]
    fn live_request_id_collision_is_rejected_instead_of_reused() {
        let owner = OwnerToken::transitional();
        let mut state = ExternalXtraState::default();
        let _first = state
            .begin_load(owner.clone(), "test-owner".to_owned(), "FirstXtra")
            .unwrap();
        state.next_request_id = 1;
        assert!(state
            .begin_load(owner, "test-owner".to_owned(), "SecondXtra")
            .is_err());
    }

    #[test]
    fn request_id_counter_exhaustion_does_not_wrap() {
        let owner = OwnerToken::transitional();
        let mut state = ExternalXtraState::default();
        state.next_request_id = u64::MAX;
        assert!(state
            .begin_load(owner, "test-owner".to_owned(), "Exhausted")
            .is_err());
        assert_eq!(state.next_request_id, u64::MAX);
    }

    #[test]
    fn reset_rotates_capability_nonce_before_reusing_request_id() {
        let owner = OwnerToken::transitional();
        let mut state = ExternalXtraState::default();
        let old = state
            .begin_load(owner.clone(), "owner".to_owned(), "ExampleXtra")
            .unwrap();
        state.reset();
        let replacement = state
            .begin_load(owner.clone(), "owner".to_owned(), "ExampleXtra")
            .unwrap();
        assert_eq!(old.request_id, replacement.request_id);
        assert_ne!(old.state_id, replacement.state_id);
        state.complete_load(
            &replacement.owner,
            old.state_id,
            old.request_id,
            "ExampleXtra",
            true,
        );
        assert!(state.take_load_waiter(&replacement).is_some());
    }

    #[test]
    fn nested_host_requests_preserve_operation_identity_and_wire_args() {
        let owner = OwnerToken::transitional();
        let (command_tx, _) = async_std::channel::unbounded();
        let mut player = DirPlayer::new_with_owner(command_tx, owner.clone());
        player.xtra_manager_state.external.register("Nested");
        let args = vec![
            XDatum::String("Nested".to_owned()),
            XDatum::List(vec![XDatum::Int(7), XDatum::String("payload".to_owned())]),
        ];
        let request = prepare_nested_host_request(&player, 5, &args).unwrap();
        assert_eq!(request.xtra_name, "Nested");
        assert!(matches!(request.operation, ExternalXtraOperation::Create));
        let encoded = wire::decode_args(&request.args).unwrap();
        assert!(matches!(encoded.as_slice(), [XDatum::Int(7), XDatum::String(value)] if value == "payload"));
        assert!(request.owner.same_identity(&owner));

        let call = prepare_nested_host_request(
            &player,
            6,
            &[
                XDatum::String("Nested".to_owned()),
                XDatum::Int(2),
                XDatum::String("handle".to_owned()),
                XDatum::List(vec![XDatum::Int(9)]),
            ],
        )
        .unwrap();
        assert!(matches!(
            call.operation,
            ExternalXtraOperation::Instance { instance_id: 2, ref handler } if handler == "handle"
        ));
        let destroy = prepare_nested_host_request(
            &player,
            7,
            &[XDatum::String("Nested".to_owned()), XDatum::Int(2)],
        )
        .unwrap();
        assert!(matches!(
            destroy.operation,
            ExternalXtraOperation::Destroy { instance_id: 2 }
        ));
    }

    #[test]
    fn borrowed_host_dispatch_refuses_plugin_reentry_boundary() {
        let owner = OwnerToken::transitional();
        let (command_tx, _) = async_std::channel::unbounded();
        let mut player = DirPlayer::new_with_owner(command_tx, owner);
        let mut symbols = SymbolTable::new();
        player.xtra_manager_state.external.register("Nested");
        let args = wire::encode_args(&[
            XDatum::String("Nested".to_owned()),
            XDatum::List(vec![XDatum::Int(1)]),
        ]);
        let result = host_call_dispatch(&mut player, &mut symbols, 5, &args);
        assert_eq!(
            result,
            wire::encode_error("external Xtra host calls require the owner-bound executor")
        );
    }
}

// Suppress unused-import warnings when the wasm32 cfg block isn't active.
#[allow(dead_code)]
fn _suppress() {
    let _ = HashMap::<i32, i32>::new();
}
