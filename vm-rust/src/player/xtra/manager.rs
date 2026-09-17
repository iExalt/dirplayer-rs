use crate::{
    director::lingo::datum::{Datum, XtraInstanceId},
    director::static_datum::StaticDatum,
    player::{DatumRef, ScriptError, ownership::OwnerToken},
};

/// Fully-owned transport payloads. Deferred executors consume these after
/// revalidating the instance identity; no arena-backed argument refs cross an
/// await boundary.
#[derive(Clone)]
pub(crate) struct MultiuserConnectRequest {
    pub(crate) username: String,
    pub(crate) password: String,
    pub(crate) host: String,
    pub(crate) port: i32,
    pub(crate) movie_id: String,
    pub(crate) mode: i32,
    pub(crate) encryption_key: String,
    /// Captured before the VM borrow ends.  Browser hosts may override the
    /// page protocol and socket path per player; reading those settings from
    /// a later callback would route a reused player through the wrong host.
    pub(crate) websocket_path: Option<String>,
    pub(crate) websocket_ssl: Option<bool>,
}

#[derive(Clone)]
pub(crate) enum MultiuserSendRequest {
    Text { message: String },
    Binary {
        recipients: Vec<String>,
        subject: String,
        content: StaticDatum,
        sender_id: String,
    },
}

#[derive(Clone)]
pub(crate) struct CurlExecRequest {
    pub(crate) instance: crate::player::xtra::curl::CurlInstance,
    pub(crate) return_mode: i32,
}

#[derive(Clone)]
pub(crate) struct FileIoOpenRequest {
    pub(crate) owner: OwnerToken,
    pub(crate) instance_id: XtraInstanceId,
    pub(crate) generation: u64,
    pub(crate) file_name: String,
    pub(crate) relative_name: String,
    /// NetManager task allocated while the openFile invocation still held
    /// the player borrow. The task owns the resolved URL, so execution never
    /// re-reads mutable base-path/override configuration after an await.
    pub(crate) prepared: crate::player::net_manager::PreparedNetTask,
    pub(crate) mode: i32,
}

use super::budapi::{BudApiHostIntent, BudApiState, BudApiXtra};
use super::curl::{CurlXtra, CurlXtraManager};
use super::external;
use super::fileio::FileIoXtraManager;
use super::leechprotection::LeechProtectionXtra;
use super::multiuser::MultiuserXtraManager;
use super::openurl::{OpenUrlHostIntent, OpenUrlXtra};
use super::sysmenu::{SysMenuHostIntent, SysMenuManager, SysMenuXtra};
use super::xmlparser::XmlParserXtraManager;

/// A network operation that has crossed the synchronous Xtra boundary. The
/// request owns all arguments and carries both the player capability and the
/// instance generation; a later session executor must validate both before it
/// touches a socket or writes a completion.
#[derive(Clone)]
pub(crate) enum XtraPendingIntent {
    MultiuserConnect {
        owner: OwnerToken,
        instance_id: XtraInstanceId,
        generation: u64,
        request: MultiuserConnectRequest,
    },
    MultiuserSend {
        owner: OwnerToken,
        instance_id: XtraInstanceId,
        generation: u64,
        request: MultiuserSendRequest,
    },
    CurlExec {
        owner: OwnerToken,
        instance_id: XtraInstanceId,
        generation: u64,
        request: CurlExecRequest,
    },
    FileIoOpen(FileIoOpenRequest),
    SysMenu(SysMenuHostIntent),
    BudApi(BudApiHostIntent),
    OpenUrl(OpenUrlHostIntent),
}

pub(crate) enum XtraPendingOrValue {
    Value(DatumRef),
    Pending(XtraPendingIntent),
}

impl XtraPendingIntent {
    pub(crate) fn owner(&self) -> &OwnerToken {
        match self {
            Self::MultiuserConnect { owner, .. }
            | Self::MultiuserSend { owner, .. }
            | Self::CurlExec { owner, .. } => owner,
            Self::FileIoOpen(request) => &request.owner,
            Self::SysMenu(request) => request.owner(),
            Self::BudApi(request) => request.owner(),
            Self::OpenUrl(request) => &request.owner,
        }
    }

    pub(crate) fn instance(&self) -> (XtraInstanceId, u64) {
        match self {
            Self::MultiuserConnect { instance_id, generation, .. }
            | Self::MultiuserSend { instance_id, generation, .. }
            | Self::CurlExec { instance_id, generation, .. } => (*instance_id, *generation),
            Self::FileIoOpen(request) => (request.instance_id, request.generation),
            Self::SysMenu(_) | Self::BudApi(_) | Self::OpenUrl(_) => (0, 0),
        }
    }

    /// A cheap preflight check for callers that already hold the expected
    /// owner and generation. This does not establish that the instance still
    /// exists; executors must call `XtraManagerState::validate_pending_intent`
    /// against the current manager maps before using the request.
    pub(crate) fn invalidated(&self, owner: &OwnerToken, generation: u64) -> bool {
        !self.owner().same_identity(owner)
            || !self.owner().is_arena_live()
            || (self.instance().1 != generation
                && !matches!(self, Self::SysMenu(_) | Self::BudApi(_) | Self::OpenUrl(_)))
    }
}

/// Teardown work is retained until the session-owned executor can release
/// host resources. Dropping the local socket sender is still immediate, but a
/// queued request lets a host close operation observe the exact old identity.
pub(crate) struct XtraTeardownRequest {
    pub(crate) xtra_name: &'static str,
    pub(crate) owner: OwnerToken,
    pub(crate) instance_id: XtraInstanceId,
    pub(crate) generation: u64,
    /// Browser socket resources are moved here while the player borrow is
    /// still held and are dropped by the owner/session teardown boundary
    /// after that borrow ends. Native requests carry no host resource.
    #[cfg(target_arch = "wasm32")]
    pub(crate) socket_resource: Option<super::multiuser::MultiuserSocketResource>,
    #[cfg(test)]
    pub(crate) drop_probe: Option<std::rc::Rc<dyn Fn()>>,
}

#[cfg(test)]
impl Drop for XtraTeardownRequest {
    fn drop(&mut self) {
        if let Some(probe) = self.drop_probe.take() {
            probe();
        }
    }
}

/// Xtra state bound to one player owner. Instance numbers are only meaningful
/// inside this state; canonical dispatch never consults a process-global map.
pub struct XtraManagerState {
    pub(crate) owner: OwnerToken,
    pub(crate) external: external::ExternalXtraState,
    pub(crate) multiuser: MultiuserXtraManager,
    pub(crate) curl: CurlXtraManager,
    pub(crate) fileio: FileIoXtraManager,
    pub(crate) xmlparser: XmlParserXtraManager,
    pub(crate) sysmenu: SysMenuManager,
    pub(crate) budapi: BudApiState,
    pub(crate) teardown_requests: Vec<XtraTeardownRequest>,
}

impl XtraManagerState {
    pub(crate) fn new(owner: OwnerToken) -> Self {
        Self {
            owner: owner.clone(),
            external: external::ExternalXtraState::default(),
            multiuser: MultiuserXtraManager::new_with_owner(owner.clone()),
            curl: CurlXtraManager::new_with_owner(owner.clone()),
            fileio: FileIoXtraManager::new_with_owner(owner.clone()),
            xmlparser: XmlParserXtraManager::new_with_owner(owner.clone()),
            sysmenu: SysMenuManager::new(owner),
            budapi: BudApiState::default(),
            teardown_requests: Vec::new(),
        }
    }

    pub(crate) fn reset(&mut self) {
        self.external.reset();
        self.multiuser.reset();
        self.curl.reset();
        self.fileio.reset();
        self.xmlparser.reset();
        self.sysmenu.reset();
        self.budapi = BudApiState::default();
        self.teardown_requests
            .extend(self.multiuser.take_teardown_requests());
        self.teardown_requests.extend(self.curl.take_teardown_requests());
    }

    pub(crate) fn rebind_owner(&mut self, owner: OwnerToken) {
        self.owner = owner.clone();
        self.external.cancel_pending_loads();
        self.multiuser.rebind_owner(owner.clone());
        self.curl.rebind_owner(owner.clone());
        self.fileio.rebind_owner(owner.clone());
        self.xmlparser.rebind_owner(owner.clone());
        self.sysmenu.rebind_owner(owner);
        self.budapi = BudApiState::default();
    }

    pub(crate) fn take_teardown_requests(&mut self) -> Vec<XtraTeardownRequest> {
        self.teardown_requests
            .extend(self.multiuser.take_teardown_requests());
        self.teardown_requests.extend(self.curl.take_teardown_requests());
        std::mem::take(&mut self.teardown_requests)
    }

    /// Validate a deferred operation against the live, owner-bound instance
    /// map. The generation carried by an intent is only meaningful together
    /// with this lookup: after close/reset and numeric ID reuse, the old
    /// request must be rejected even when its ID is the same.
    pub(crate) fn validate_pending_intent(
        &self,
        intent: &XtraPendingIntent,
    ) -> Result<(), ScriptError> {
        let (manager_owner, current_generation) = match intent {
            XtraPendingIntent::MultiuserConnect {
                instance_id,
                ..
            }
            | XtraPendingIntent::MultiuserSend {
                instance_id,
                ..
            } => (
                &self.multiuser.owner,
                self.multiuser.instance_generation(*instance_id),
            ),
            XtraPendingIntent::CurlExec {
                instance_id,
                ..
            } => (
                &self.curl.owner,
                self.curl.instance_generation(*instance_id),
            ),
            XtraPendingIntent::FileIoOpen(request) => (
                &self.fileio.owner,
                self.fileio.instance_generation(request.instance_id),
            ),
            XtraPendingIntent::SysMenu(request) => {
                if !self.owner.same_identity(request.owner())
                    || !request.owner().is_arena_live()
                {
                    return Err(ScriptError::new_code(
                        crate::player::ScriptErrorCode::InvalidReference,
                        "stale or foreign SysMenu pending intent".to_owned(),
                    ));
                }
                return Ok(());
            }
            XtraPendingIntent::BudApi(request) => {
                if !self.owner.same_identity(request.owner())
                    || !request.owner().is_arena_live()
                {
                    return Err(ScriptError::new_code(
                        crate::player::ScriptErrorCode::InvalidReference,
                        "stale BudAPI host intent".to_owned(),
                    ));
                }
                return Ok(());
            }
            XtraPendingIntent::OpenUrl(request) => {
                if !self.owner.same_identity(&request.owner)
                    || !request.owner.is_arena_live()
                {
                    return Err(ScriptError::new_code(
                        crate::player::ScriptErrorCode::InvalidReference,
                        "stale OpenURL host intent".to_owned(),
                    ));
                }
                return Ok(());
            }
        };
        let (_, expected_generation) = intent.instance();
        if !manager_owner.same_identity(intent.owner())
            || !manager_owner.is_arena_live()
            || current_generation != Some(expected_generation)
        {
            return Err(ScriptError::new_code(
                crate::player::ScriptErrorCode::InvalidReference,
                "stale or foreign Xtra pending intent".to_owned(),
            ));
        }
        Ok(())
    }

}

/// Canonical synchronous instance dispatch for the owner-bound Xtras. Other
/// Xtras retain their existing manager boundary until their own migration.
pub(crate) fn call_instance_handler_explicit(
    player: &mut crate::player::DirPlayer,
    symbols: &mut crate::player::symbols::symbol_table::SymbolTable,
    xtra_name: &str,
    receiver: &DatumRef,
    handler_name: &str,
    args: &[DatumRef],
) -> Result<DatumRef, ScriptError> {
    let instance_id = match receiver {
        DatumRef::Void => {
            return Err(ScriptError::new_code(
                crate::player::ScriptErrorCode::InvalidReference,
                "missing Xtra instance receiver".to_owned(),
            ))
        }
        _ => match player.allocator.try_get_datum(receiver) {
            Some(Datum::XtraInstance(name, id))
                if name.eq_ignore_ascii_case(xtra_name) => *id,
            Some(Datum::XtraInstance(_, _)) => {
                return Err(ScriptError::new_code(
                    crate::player::ScriptErrorCode::InvalidReference,
                    "Xtra receiver name does not match its dispatch owner".to_owned(),
                ))
            }
            _ => {
                return Err(ScriptError::new_code(
                    crate::player::ScriptErrorCode::InvalidReference,
                    "foreign or stale Xtra instance receiver".to_owned(),
                ))
            }
        },
    };
    let xtra_name_lower = xtra_name.to_ascii_lowercase();
    // Classify deferred operations before entering a manager. This boundary
    // is synchronous, so it must not prepare a request and then discard it;
    // the pending API below is the only route that retains owned transport
    // data for the session executor.
    if (xtra_name_lower == "multiuser"
        && (handler_name.eq_ignore_ascii_case("connectToNetServer")
            || handler_name.eq_ignore_ascii_case("sendNetMessage")))
        || (xtra_name_lower == "curl" && handler_name.eq_ignore_ascii_case("execAsync"))
    {
        return Err(ScriptError::new(
            "deferred Xtra dispatch requires the pending executor boundary".to_owned(),
        ));
    }
    match xtra_name_lower.as_str() {
        "multiuser" => player.with_xtra_manager_state(|state, player| {
            let outcome = state.multiuser.call_instance_handler_or_pending_explicit(
                player, symbols, instance_id, handler_name, args,
            )?;
            match outcome {
                XtraPendingOrValue::Value(value) => Ok(value),
                XtraPendingOrValue::Pending(_) => Err(ScriptError::new(
                    "deferred Multiuser dispatch reached the synchronous boundary".to_owned(),
                )),
            }
        }),
        "curl" => player.with_xtra_manager_state(|state, player| {
            let outcome = state.curl.call_instance_handler_or_pending_explicit(
                player, symbols, instance_id, handler_name, args,
            )?;
            match outcome {
                XtraPendingOrValue::Value(value) => Ok(value),
                XtraPendingOrValue::Pending(_) => Err(ScriptError::new(
                    "deferred Curl dispatch reached the synchronous boundary".to_owned(),
                )),
            }
        }),
        "fileio" => player.with_xtra_manager_state(|state, player| {
            state.fileio.call_instance_handler_explicit(
                player, symbols, instance_id, handler_name, args,
            )
        }),
        "xmlparser" => player.with_xtra_manager_state(|state, player| {
            state.xmlparser.call_instance_handler_explicit(
                player, symbols, instance_id, handler_name, args,
            )
        }),
        _ => Err(ScriptError::new(format!(
            "Xtra {} requires its explicit owner migration",
            xtra_name
        ))),
    }
}

/// Context checked dispatch used by the session executor when it can suspend
/// the current turn. Unlike the compatibility Result-returning wrapper above,
/// this preserves a prepared network intent for the caller to enqueue.
pub(crate) fn call_instance_handler_pending_explicit(
    player: &mut crate::player::DirPlayer,
    symbols: &mut crate::player::symbols::symbol_table::SymbolTable,
    xtra_name: &str,
    receiver: &DatumRef,
    handler_name: &str,
    args: &[DatumRef],
) -> Result<XtraPendingOrValue, ScriptError> {
    let instance_id = match player.allocator.try_get_datum(receiver) {
        Some(Datum::XtraInstance(name, id)) if name.eq_ignore_ascii_case(xtra_name) => *id,
        _ => return Err(ScriptError::new_code(
            crate::player::ScriptErrorCode::InvalidReference,
            "foreign or stale Xtra instance receiver".to_owned(),
        )),
    };
    match xtra_name.to_ascii_lowercase().as_str() {
        "multiuser" => player.with_xtra_manager_state(|state, player| {
            state.multiuser.call_instance_handler_or_pending_explicit(
                player, symbols, instance_id, handler_name, args,
            )
        }),
        "curl" => player.with_xtra_manager_state(|state, player| {
            state.curl.call_instance_handler_or_pending_explicit(
                player, symbols, instance_id, handler_name, args,
            )
        }),
        "fileio" => player.with_xtra_manager_state(|state, player| {
            state.fileio.call_instance_handler_pending_explicit(
                player, symbols, instance_id, handler_name, args,
            )
        }),
        "xmlparser" => player.with_xtra_manager_state(|state, player| {
            state.xmlparser.call_instance_handler_explicit(
                player, symbols, instance_id, handler_name, args,
            ).map(XtraPendingOrValue::Value)
        }),
        _ => Err(ScriptError::new(format!(
            "Xtra {} requires its explicit owner migration", xtra_name
        ))),
    }
}

/// Execute a SysMenu browser effect outside the VM borrow. The callback is
/// injectable so tests can re-enter/reset the session and prove the completion
/// fence is checked after the host call.
pub(crate) fn execute_sysmenu_host_intent_with<F>(
    session: &std::rc::Rc<std::cell::RefCell<crate::player::session::RuntimeSession>>,
    player_id: crate::player::session::PlayerId,
    intent: SysMenuHostIntent,
    host: F,
) -> Result<DatumRef, ScriptError>
where
    F: FnOnce(&SysMenuHostIntent),
{
    let pending = XtraPendingIntent::SysMenu(intent.clone());
    let valid = session.borrow_mut().with_player(player_id, |context| {
        context.player.xtra_manager_state.validate_pending_intent(&pending)
    });
    match valid {
        Some(Ok(())) => {}
        Some(Err(error)) => return Err(error),
        None => return Err(crate::player::cancelled_scope_error()),
    }
    host(&intent);
    let owner = intent.owner().clone();
    session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(crate::player::cancelled_scope_error());
            }
            Ok(context.player.alloc_datum(Datum::Int(1)))
        })
        .unwrap_or_else(|| Err(crate::player::cancelled_scope_error()))
}

pub(crate) fn execute_budapi_host_intent_with<F>(
    session: &std::rc::Rc<std::cell::RefCell<crate::player::session::RuntimeSession>>,
    player_id: crate::player::session::PlayerId,
    intent: BudApiHostIntent,
    host: F,
) -> Result<DatumRef, ScriptError>
where
    F: FnOnce(&BudApiHostIntent) -> bool,
{
    let pending = XtraPendingIntent::BudApi(intent.clone());
    let valid = session.borrow_mut().with_player(player_id, |context| {
        context.player.xtra_manager_state.validate_pending_intent(&pending)
    });
    match valid {
        Some(Ok(())) => {}
        Some(Err(error)) => return Err(error),
        None => return Err(crate::player::cancelled_scope_error()),
    }
    let success = host(&intent);
    session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !intent.owner().same_identity(&context.player.owner)
                || !intent.owner().is_arena_live()
            {
                return Err(crate::player::cancelled_scope_error());
            }
            Ok(context.player.alloc_datum(Datum::Int(if success { 1 } else { 0 })))
        })
        .unwrap_or_else(|| Err(crate::player::cancelled_scope_error()))
}

fn execute_budapi_clipboard_read(
    session: &std::rc::Rc<std::cell::RefCell<crate::player::session::RuntimeSession>>,
    player_id: crate::player::session::PlayerId,
    intent: BudApiHostIntent,
) -> Result<DatumRef, ScriptError> {
    let pending = XtraPendingIntent::BudApi(intent.clone());
    let valid = session.borrow_mut().with_player(player_id, |context| {
        context.player.xtra_manager_state.validate_pending_intent(&pending)
    });
    match valid {
        Some(Ok(())) => {}
        Some(Err(error)) => return Err(error),
        None => return Err(crate::player::cancelled_scope_error()),
    }
    session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !intent.owner().same_identity(&context.player.owner)
                || !intent.owner().is_arena_live()
            {
                return Err(crate::player::cancelled_scope_error());
            }
            let text = context.player.xtra_manager_state.budapi.clipboard_text.clone();
            Ok(context.player.alloc_datum(Datum::String(text)))
        })
        .unwrap_or_else(|| Err(crate::player::cancelled_scope_error()))
}

async fn execute_budapi_clipboard_write(
    session: &std::rc::Rc<std::cell::RefCell<crate::player::session::RuntimeSession>>,
    player_id: crate::player::session::PlayerId,
    intent: BudApiHostIntent,
) -> Result<DatumRef, ScriptError> {
    let pending = XtraPendingIntent::BudApi(intent.clone());
    let valid = session.borrow_mut().with_player(player_id, |context| {
        context.player.xtra_manager_state.validate_pending_intent(&pending)
    });
    match valid {
        Some(Ok(())) => {}
        Some(Err(error)) => return Err(error),
        None => return Err(crate::player::cancelled_scope_error()),
    }
    #[cfg(target_arch = "wasm32")]
    let _clipboard_ok = {
        let text = match &intent {
            BudApiHostIntent::ClipboardWrite { text, .. } => text,
            _ => unreachable!("clipboard write executor received another intent"),
        };
        // The owner-local fallback is committed only after the host promise
        // settles and the owner fence below succeeds. This keeps a retired
        // generation from writing clipboard state into its replacement.
        write_clipboard_best_effort(text).await
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = intent;
        return Err(ScriptError::new(
            "BudAPI clipboard write requires the browser host executor".to_owned(),
        ));
    }
    #[cfg(target_arch = "wasm32")]
    session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !intent.owner().same_identity(&context.player.owner)
                || !intent.owner().is_arena_live()
            {
                return Err(crate::player::cancelled_scope_error());
            }
            let text = match &intent {
                BudApiHostIntent::ClipboardWrite { text, .. } => text.clone(),
                _ => unreachable!("clipboard write executor received another intent"),
            };
            context.player.xtra_manager_state.budapi.clipboard_text = text;
            Ok(context.player.alloc_datum(Datum::Int(1)))
        })
        .unwrap_or_else(|| Err(crate::player::cancelled_scope_error()))
}

/// Call the browser clipboard API without allowing a missing/insecure
/// capability to throw through the WASM boundary.  The owner-local fallback
/// is committed by the caller only after this operation and its owner fence.
#[cfg(target_arch = "wasm32")]
async fn write_clipboard_best_effort(text: &str) -> bool {
    use wasm_bindgen::{JsCast, JsValue};

    let Some(window) = web_sys::window() else {
        return false;
    };
    let navigator = window.navigator();
    let clipboard = match js_sys::Reflect::get(
        navigator.as_ref(),
        &JsValue::from_str("clipboard"),
    ) {
        Ok(value) if !value.is_null() && !value.is_undefined() => value,
        _ => return false,
    };
    let write_text = match js_sys::Reflect::get(&clipboard, &JsValue::from_str("writeText"))
        .ok()
        .and_then(|value| value.dyn_into::<js_sys::Function>().ok())
    {
        Some(function) => function,
        None => return false,
    };
    let promise = match write_text.call1(&clipboard, &JsValue::from_str(text)) {
        Ok(value) => match value.dyn_into::<js_sys::Promise>() {
            Ok(promise) => promise,
            Err(_) => return false,
        },
        Err(_) => return false,
    };
    wasm_bindgen_futures::JsFuture::from(promise).await.is_ok()
}

pub(crate) fn execute_openurl_host_intent_with<F>(
    session: &std::rc::Rc<std::cell::RefCell<crate::player::session::RuntimeSession>>,
    player_id: crate::player::session::PlayerId,
    intent: OpenUrlHostIntent,
    host: F,
) -> Result<DatumRef, ScriptError>
where
    F: FnOnce(&OpenUrlHostIntent) -> bool,
{
    let pending = XtraPendingIntent::OpenUrl(intent.clone());
    let valid = session.borrow_mut().with_player(player_id, |context| {
        context.player.xtra_manager_state.validate_pending_intent(&pending)
    });
    match valid {
        Some(Ok(())) => {}
        Some(Err(error)) => return Err(error),
        None => return Err(crate::player::cancelled_scope_error()),
    }
    let success = host(&intent);
    session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !intent.owner.same_identity(&context.player.owner)
                || !intent.owner.is_arena_live()
            {
                return Err(crate::player::cancelled_scope_error());
            }
            Ok(context.player.alloc_datum(Datum::Int(if success { 1 } else { 0 })))
        })
        .unwrap_or_else(|| Err(crate::player::cancelled_scope_error()))
}

/// Execute an owner-validated transport intent outside the VM borrow.  The
/// command/evaluator layer owns the await boundary; this function is the one
/// place where the typed intent is handed to a transport implementation.
/// Browser-specific transports currently return their completion through the
/// owner-qualified callback command queue, while native builds report the
/// capability as unavailable instead of leaving a pending action retained.
pub(crate) async fn execute_pending_intent(
    session: &std::rc::Rc<std::cell::RefCell<crate::player::session::RuntimeSession>>,
    player_id: crate::player::session::PlayerId,
    intent: XtraPendingIntent,
) -> Result<DatumRef, ScriptError> {
    match intent {
        XtraPendingIntent::SysMenu(request) => {
            execute_sysmenu_host_intent_with(session, player_id, request, |request| match request {
                SysMenuHostIntent::Print { message, .. } => {
                    #[cfg(target_arch = "wasm32")]
                    web_sys::console::log_1(&format!("[SysMenu] {}", message).into());
                    #[cfg(not(target_arch = "wasm32"))]
                    log::info!("[SysMenu] {}", message);
                }
                SysMenuHostIntent::MessageBox { message, caption, message_type, .. } => {
                    let text = if caption.is_empty() {
                        message.to_owned()
                    } else {
                        format!("{}\n\n{}", caption, message)
                    };
                    let _ = message_type;
                    #[cfg(target_arch = "wasm32")]
                    if let Some(window) = web_sys::window() {
                        let _ = window.alert_with_message(&text);
                    }
                    #[cfg(not(target_arch = "wasm32"))]
                    log::info!("[SysMenu message box] {}", text);
                }
            })
        }
        XtraPendingIntent::BudApi(request) => {
            if matches!(&request, BudApiHostIntent::ClipboardRead { .. }) {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let _ = request;
                    return Err(ScriptError::new(
                        "BudAPI clipboard read requires the browser host executor".to_owned(),
                    ));
                }
                #[cfg(target_arch = "wasm32")]
                return execute_budapi_clipboard_read(session, player_id, request);
            }
            if matches!(&request, BudApiHostIntent::ClipboardWrite { .. }) {
                return execute_budapi_clipboard_write(session, player_id, request).await;
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                return Err(ScriptError::new(
                    "BudAPI browser host effect requires the browser executor".to_owned(),
                ));
            }
            execute_budapi_host_intent_with(session, player_id, request, |request| match request {
                BudApiHostIntent::Open { target, .. } => {
                    #[cfg(target_arch = "wasm32")]
                    { web_sys::window().map(|window| window.open_with_url_and_target(target, "_blank").is_ok()).unwrap_or(false) }
                    #[cfg(not(target_arch = "wasm32"))]
                    { let _ = target; unreachable!("BudAPI open requires the browser executor") }
                }
                BudApiHostIntent::Alert { text, .. } => {
                    #[cfg(target_arch = "wasm32")]
                    { web_sys::window().map(|window| window.alert_with_message(text).is_ok()).unwrap_or(false) }
                    #[cfg(not(target_arch = "wasm32"))]
                    { let _ = text; unreachable!("BudAPI alert requires the browser executor") }
                }
                BudApiHostIntent::ClipboardWrite { text, .. } => {
                    let _ = text;
                    unreachable!("clipboard writes use their async completion path")
                }
                BudApiHostIntent::ClipboardRead { .. } => unreachable!("clipboard reads use their string completion path"),
            })
        }
        XtraPendingIntent::OpenUrl(request) => {
            #[cfg(not(target_arch = "wasm32"))]
            {
                let _ = request;
                return Err(ScriptError::new(
                    "OpenURL browser host effect requires the browser executor".to_owned(),
                ));
            }
            #[cfg(target_arch = "wasm32")]
            execute_openurl_host_intent_with(session, player_id, request, |request| {
                #[cfg(target_arch = "wasm32")]
                { web_sys::window().map(|window| window.open_with_url_and_target(&request.url, "_blank").is_ok()).unwrap_or(false) }
            })
        }
        XtraPendingIntent::CurlExec {
            owner,
            instance_id,
            generation,
            request,
        } => {
            let result = super::curl::perform_fetch_owned(&request.instance).await;
            let (status, body, headers) = result;
            let callback_headers = headers.clone();
            let applied = session.borrow_mut().with_player(player_id, |context| {
                if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                    return Err(crate::player::cancelled_scope_error());
                }
                if !context.player.xtra_manager_state.curl.owner.same_identity(&owner)
                    || context.player.xtra_manager_state.curl.instance_generation(instance_id) != Some(generation)
                {
                    return Err(ScriptError::new_code(
                        crate::player::ScriptErrorCode::InvalidReference,
                        "stale or foreign Curl completion".to_owned(),
                    ));
                }
                context.player.xtra_manager_state.curl.apply_owned_result(
                    instance_id,
                    generation,
                    status,
                    body.len(),
                    headers,
                    request.instance.requested_url().to_owned(),
                )?;
                let result = context.player.alloc_datum(Datum::Int(if status > 0 { 0 } else { status }));
                let callback = context.player.xtra_manager_state.curl.completion_callback(instance_id, generation)?;
                let queue = context.player.queue_tx.clone();
                if let Some((target, handler)) = callback {
                    let body_ref = context.player.alloc_datum(Datum::String(
                        body.iter().map(|byte| *byte as char).collect(),
                    ));
                    let status_ref = context.player.alloc_datum(Datum::Int(status));
                    let headers_ref = context.player.alloc_datum(Datum::String(
                        callback_headers.clone(),
                    ));
                    let args = if request.return_mode == 1 {
                        vec![body_ref, status_ref, headers_ref]
                    } else {
                        vec![status_ref, body_ref, headers_ref]
                    };
                    let handler_name = context.symbols.intern(&handler);
                    let _ = queue.try_send(crate::player::PlayerVMExecutionItem {
                        command: crate::player::commands::PlayerVMCommand::TriggerXtraCallback {
                            owner: owner.clone(),
                            target,
                            handler_name,
                            args,
                        },
                        completer: None,
                    });
                }
                Ok(result)
            });
            applied.unwrap_or_else(|| Err(crate::player::cancelled_scope_error()))
        }
        XtraPendingIntent::FileIoOpen(request) => {
            let wait = session
                .borrow_mut()
                .with_player(player_id, |context| {
                    if !owner_matches_fileio(context.player, &request.owner, request.instance_id, request.generation) {
                        return Err(crate::player::cancelled_scope_error());
                    }
                    context
                        .player
                        .net_manager
                        .get_task(request.prepared.task.id)
                        .filter(|task| task.resolved_url == request.prepared.task.resolved_url)
                        .ok_or_else(crate::player::cancelled_scope_error)?;
                    Ok(context.player.net_manager.create_task_future(request.prepared.task.id))
                })
                .ok_or_else(crate::player::cancelled_scope_error)??;
            crate::player::net_manager::NetManager::execute_prepared_task(request.prepared.clone()).await;
            wait.await;
            let applied = session.borrow_mut().with_player(player_id, |context| {
                if !owner_matches_fileio(context.player, &request.owner, request.instance_id, request.generation) {
                    return Err(crate::player::cancelled_scope_error());
                }
                let result = context
                    .player
                    .net_manager
                    .get_task_result(Some(request.prepared.task.id));
                let instance = context
                    .player
                    .xtra_manager_state
                    .fileio
                    .instances
                    .get_mut(&request.instance_id)
                    .ok_or_else(|| ScriptError::new_code(
                        crate::player::ScriptErrorCode::InvalidReference,
                        "stale or foreign FileIO completion".to_owned(),
                    ))?;
                match result {
                    Some(Ok(bytes)) => {
                        instance.file_name = request.file_name.clone();
                        instance.data = bytes;
                        instance.position = 0;
                        instance.is_open = true;
                        instance.last_error = 0;
                    }
                    Some(Err(_error)) => {
                        instance.file_name = request.file_name.clone();
                        instance.data.clear();
                        instance.position = 0;
                        instance.is_open = true;
                        instance.last_error = super::fileio::remote_open_error(request.mode);
                    }
                    None => {
                        return Err(ScriptError::new(
                            "FileIO openFile task completed without a result".to_owned(),
                        ));
                    }
                }
                Ok(context.player.alloc_datum(Datum::Void))
            });
            applied.unwrap_or_else(|| Err(crate::player::cancelled_scope_error()))
        }
        XtraPendingIntent::MultiuserSend {
            owner,
            instance_id,
            generation,
            request,
        } => {
            session
                .borrow_mut()
                .with_player(player_id, |context| {
                    if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                        return Err(crate::player::cancelled_scope_error());
                    }
                    context.player.xtra_manager_state.multiuser.send_owned(
                        instance_id,
                        generation,
                        request,
                    )
                })
                .unwrap_or_else(|| Err(crate::player::cancelled_scope_error()))
                .map(|()| DatumRef::Void)
        }
        XtraPendingIntent::MultiuserConnect {
            owner,
            instance_id,
            generation,
            request,
        } => {
            let preparation = session
                .borrow_mut()
                .with_player(player_id, |context| {
                    if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                        return Err(crate::player::cancelled_scope_error());
                    }
                    context.player.xtra_manager_state.multiuser.prepare_connect_owned(
                        instance_id,
                        generation,
                        owner.clone(),
                        request,
                        context.player.queue_tx.clone(),
                    )
                })
                .and_then(Result::ok)
                .ok_or_else(crate::player::cancelled_scope_error)?;
            #[cfg(target_arch = "wasm32")]
            {
                let mut connected = Some(super::multiuser::MultiuserXtraManager::execute_connect_owned(&preparation)?);
                let install_result = session
                    .borrow_mut()
                    .with_player(player_id, |context| {
                        if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                            return Err(crate::player::cancelled_scope_error());
                        }
                        context.player.xtra_manager_state.multiuser.install_connect_owned(
                            &preparation,
                            &mut connected,
                        )
                    });
                let retired = match install_result {
                    Some(Ok(retired)) => retired,
                    Some(Err(error)) => {
                        drop(connected);
                        return Err(error);
                    }
                    None => {
                        drop(connected);
                        return Err(crate::player::cancelled_scope_error());
                    }
                };
                // Any rejected or replaced browser resource is dropped only
                // after the short RuntimeSession borrow has ended.
                drop(connected);
                drop(retired);
                Ok(DatumRef::Void)
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let _ = preparation;
                Err(ScriptError::new("Multiuser WebSocket executor unavailable on native target".to_owned()))
            }
        }
    }
}

fn owner_matches_fileio(
    player: &crate::player::DirPlayer,
    owner: &OwnerToken,
    instance_id: XtraInstanceId,
    generation: u64,
) -> bool {
    owner.same_identity(&player.owner)
        && owner.is_arena_live()
        && player.xtra_manager_state.fileio.owner.same_identity(owner)
        && player.xtra_manager_state.fileio.instance_generation(instance_id) == Some(generation)
}

pub(crate) fn create_xtra_instance_explicit(
    player: &mut crate::player::DirPlayer,
    symbols: & crate::player::symbols::symbol_table::SymbolTable,
    xtra_name: &str,
    args: &[DatumRef],
) -> Result<XtraInstanceId, ScriptError> {
    // Browser-loaded plugins retain precedence over built-ins. Their
    // session-owned registry is not available through this synchronous native
    // subset, so fail explicitly rather than silently shadowing the plugin.
    if player.xtra_manager_state.external.is_registered(xtra_name) {
        return Err(ScriptError::new(format!(
            "external Xtra {} requires the session-owned plugin executor",
            xtra_name
        )));
    }
    match xtra_name.to_ascii_lowercase().as_str() {
        "multiuser" => player.with_xtra_manager_state(|state, player| {
            state.multiuser.create_instance_explicit(player, args)
        }),
        "curl" => player.with_xtra_manager_state(|state, player| {
            state.curl.create_instance_explicit(player, symbols, args)
        }),
        "fileio" => player.with_xtra_manager_state(|state, _player| {
            state.fileio.create_instance_explicit(args)
        }),
        "xmlparser" => player.with_xtra_manager_state(|state, _player| {
            state.xmlparser.create_instance_explicit(args)
        }),
        _ => Err(ScriptError::new(format!(
            "Xtra {} requires its explicit owner migration",
            xtra_name
        ))),
    }
}

pub(crate) fn try_call_xtra_static_handler_explicit(
    player: &mut crate::player::DirPlayer,
    symbols: &mut crate::player::symbols::symbol_table::SymbolTable,
    name: &str,
    args: &[DatumRef],
) -> Option<Result<DatumRef, ScriptError>> {
    if SysMenuXtra::has_handler(name) {
        return Some(player.with_xtra_manager_state(|state, player| {
            SysMenuXtra::call_handler_explicit(&mut state.sysmenu, player, symbols, name, args)
        }));
    }
    if CurlXtra::has_static_handler(name) {
        return Some(CurlXtra::call_static_handler_explicit(player, symbols, name, args));
    }
    if LeechProtectionXtra::has_handler(name) {
        return Some(LeechProtectionXtra::call_handler_explicit(player, name, args, symbols));
    }
    None
}

/// Prepare SysMenu's host effects while the owner-bound player borrow is
/// active. The returned intent owns every argument needed by the host call.
pub(crate) fn prepare_sysmenu_handler_explicit(
    player: &mut crate::player::DirPlayer,
    symbols: &crate::player::symbols::symbol_table::SymbolTable,
    name: &str,
    args: &[DatumRef],
) -> Option<Result<XtraPendingOrValue, ScriptError>> {
    if !SysMenuXtra::has_handler(name) {
        return None;
    }
    Some(player.with_xtra_manager_state(|state, player| {
        SysMenuXtra::prepare_handler(&mut state.sysmenu, player, symbols, name, args)
    }))
}

pub(crate) fn prepare_budapi_handler_explicit(
    player: &mut crate::player::DirPlayer,
    symbols: &crate::player::symbols::symbol_table::SymbolTable,
    name: &str,
    args: &[DatumRef],
) -> Option<Result<XtraPendingOrValue, ScriptError>> {
    if !BudApiXtra::has_handler(name) {
        return None;
    }
    Some(player.with_xtra_manager_state(|state, player| {
        BudApiXtra::prepare_handler(player, &mut state.budapi, symbols, name, args)
    }))
}

pub(crate) fn prepare_openurl_handler_explicit(
    player: &mut crate::player::DirPlayer,
    symbols: &crate::player::symbols::symbol_table::SymbolTable,
    name: &str,
    args: &[DatumRef],
) -> Option<Result<XtraPendingOrValue, ScriptError>> {
    if !OpenUrlXtra::has_handler(name) {
        return None;
    }
    Some(OpenUrlXtra::prepare_handler(player, name, args, symbols))
}

pub fn is_xtra_registered(player: &crate::player::DirPlayer, name: &str) -> bool {
    let name_lower = name.to_lowercase();
    // External plugin xtras win over built-ins (per the spec). This is
    // how a user-loaded plugin can shadow a stale built-in.
    if player.xtra_manager_state.external.is_registered(&name_lower) {
        return true;
    }
    return name == "Multiuser"
        || name_lower == "xmlparser"
        || name_lower == "fileio"
        || name_lower == "curl"
        || name_lower == "openurl"
        || name_lower == "sysmenu"
        || name_lower == "budapi"
        || name_lower == "leechprotectionremovalhelp";
}

pub fn get_registered_xtra_names(player: &crate::player::DirPlayer) -> Vec<String> {
    let mut names: Vec<String> = vec![
        "Multiusr".to_string(),
        "XmlParser".to_string(),
        "FileIO".to_string(),
        "Curl".to_string(),
        "OpenURL".to_string(),
        "SysMenu".to_string(),
        "BudAPI".to_string(),
        "LeechProtectionRemovalHelp".to_string(),
    ];
    // Append any externally loaded xtras. Names are stored lowercased in
    // the external registry; we surface them as-is — Director treats
    // `the xtraList` entries case-insensitively at lookup time anyway.
    names.extend(player.xtra_manager_state.external.registered_names());
    names
}

/// Dispatch for built-in static (`*`-prefixed) Xtra functions — global
/// handlers that don't require `object me` (e.g. `gsOpenURL`, `baFileExists`,
/// `sysMenuMessageBox`). External plugins use the owner-bound probe/request
/// route in `session.rs` and are deliberately absent from this synchronous
/// helper.
pub fn try_call_xtra_static_handler_with_symbols(
    player: &mut crate::player::DirPlayer,
    name: &str,
    args: &Vec<DatumRef>,
    symbols: &mut crate::player::symbols::symbol_table::SymbolTable,
) -> Option<Result<DatumRef, ScriptError>> {
    if OpenUrlXtra::has_handler(name) {
        return Some(Err(ScriptError::new(
            "OpenURL browser effect requires the owner host executor".to_owned(),
        )));
    }
    if SysMenuXtra::has_handler(name) {
        return Some(player.with_xtra_manager_state(|state, player| {
            SysMenuXtra::call_handler_explicit(&mut state.sysmenu, player, symbols, name, args)
        }));
    }
    if BudApiXtra::has_handler(name) {
        return Some(player.with_xtra_manager_state(|state, player| {
            BudApiXtra::call_handler(player, &mut state.budapi, name, args, symbols)
        }));
    }
    if CurlXtra::has_static_handler(name) {
        return Some(CurlXtra::call_static_handler_explicit(player, symbols, name, args));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_intent_is_bound_to_owner_and_instance_generation() {
        let owner = OwnerToken::transitional();
        let other_owner = OwnerToken::transitional();
        let intent = XtraPendingIntent::CurlExec {
            owner: owner.clone(),
            instance_id: 7,
            generation: 3,
            request: crate::player::xtra::manager::CurlExecRequest {
                instance: crate::player::xtra::curl::CurlInstance::new(false, 3),
                return_mode: 0,
            },
        };
        assert!(!intent.invalidated(&owner, 3));
        assert!(intent.invalidated(&owner, 4));
        assert!(intent.invalidated(&other_owner, 3));
        owner.begin_reset();
        assert!(intent.invalidated(&owner, 3));
    }

    #[test]
    fn sysmenu_host_reentry_is_borrow_free_and_late_result_is_rejected() {
        use async_std::channel;
        use std::{cell::RefCell, rc::Rc};
        use crate::player::session::RuntimeSession;
        use crate::player::symbols::symbol_table::SymbolOwner;

        let session = Rc::new(RefCell::new(RuntimeSession::new(SymbolOwner {
            session: 903,
            generation: 1,
        })));
        let (tx, _rx) = channel::unbounded();
        assert!(session.borrow_mut().add_player(1, tx));
        let owner = session
            .borrow_mut()
            .with_player(1, |context| context.player.owner.clone())
            .expect("SysMenu test player exists");
        let intent = SysMenuHostIntent::Print {
            owner: owner.clone(),
            message: "re-entry".to_owned(),
        };
        let result = execute_sysmenu_host_intent_with(
            &session,
            1,
            intent,
            |_: &SysMenuHostIntent| {
                assert!(session.try_borrow_mut().is_ok());
                session
                    .borrow_mut()
                    .reset_player_owned(1, &owner)
                    .expect("host re-entry reset should succeed");
            },
        );
        assert!(result.is_err(), "reset during host work must reject late result");
    }

    #[test]
    fn multiuser_and_curl_instances_are_local_to_each_owner() {
        use async_std::channel;
        use crate::player::session::RuntimeSession;
        use crate::player::symbols::symbol_table::SymbolOwner;

        let mut session = RuntimeSession::new(SymbolOwner {
            session: 901,
            generation: 1,
        });
        let (tx_a, _rx_a) = channel::unbounded();
        let (tx_b, _rx_b) = channel::unbounded();
        assert!(session.add_player(1, tx_a));
        assert!(session.add_player(2, tx_b));
        let owner_a = session
            .with_player(1, |context| context.player.owner.clone())
            .expect("player A exists");
        let owner_b = session
            .with_player(2, |context| context.player.owner.clone())
            .expect("player B exists");

        let (multi_a, curl_a) = session
            .with_player(1, |context| {
                context.player.with_xtra_manager_state(|state, player| {
                    (
                        state.multiuser.create_instance_explicit(player, &[]).unwrap(),
                        state
                            .curl
                            .create_instance_explicit(
                                player,
                                &crate::player::symbols::symbol_table::SymbolTable::new(),
                                &[],
                            )
                            .unwrap(),
                    )
                })
            })
            .expect("player A exists");
        let (multi_b, curl_b) = session
            .with_player(2, |context| {
                context.player.with_xtra_manager_state(|state, player| {
                    (
                        state.multiuser.create_instance_explicit(player, &[]).unwrap(),
                        state
                            .curl
                            .create_instance_explicit(
                                player,
                                &crate::player::symbols::symbol_table::SymbolTable::new(),
                                &[],
                            )
                            .unwrap(),
                    )
                })
            })
            .expect("player B exists");
        assert_eq!((multi_a, curl_a), (1, 1));
        assert_eq!((multi_b, curl_b), (1, 1));

        session.with_player(1, |context| {
            context.player.with_xtra_manager_state(|state, _| {
                assert!(state.multiuser.instance_generation(multi_a).is_some());
                assert!(state.curl.instance_generation(curl_a).is_some());
            });
        });
        session.with_player(2, |context| {
            context.player.with_xtra_manager_state(|state, _| {
                assert!(state.multiuser.instance_generation(multi_b).is_some());
                assert!(state.curl.instance_generation(curl_b).is_some());
            });
        });

        let replacement = session
            .reset_player_owned(1, &owner_a)
            .expect("owner A reset should succeed");
        assert!(!replacement.same_identity(&owner_a));
        session.with_player(1, |context| {
            context.player.with_xtra_manager_state(|state, _| {
                assert!(state.multiuser.instance_generation(multi_a).is_none());
                assert!(state.curl.instance_generation(curl_a).is_none());
            });
        });
        session.with_player(2, |context| {
            assert!(context.player.owner.same_identity(&owner_b));
            context.player.with_xtra_manager_state(|state, _| {
                assert!(state.multiuser.instance_generation(multi_b).is_some());
                assert!(state.curl.instance_generation(curl_b).is_some());
            });
        });
    }

    #[test]
    fn external_multiuser_and_curl_dispatch_respects_precedence() {
        use async_std::channel;
        use crate::director::lingo::datum::Datum;
        use crate::player::driver::{GlobalDispatch, InternalVmRequest};
        use crate::player::session::RuntimeSession;
        use crate::player::symbols::symbol_table::SymbolOwner;

        let mut session = RuntimeSession::new(SymbolOwner { session: 902, generation: 1 });
        let (tx, _rx) = channel::unbounded();
        assert!(session.add_player(1, tx));
        let (multi_receiver, curl_receiver, handler, builtin_handler) = session
            .with_player(1, |context| {
                context.player.xtra_manager_state.external.register("Multiuser");
                context.player.xtra_manager_state.external.register("Curl");
                (
                    context
                        .player
                        .alloc_datum(Datum::XtraInstance("Multiuser".to_owned(), 7)),
                    context
                        .player
                        .alloc_datum(Datum::XtraInstance("Curl".to_owned(), 9)),
                    context.symbols.intern("close"),
                    context.symbols.intern("nothing"),
                )
            })
            .expect("external-Xtra precedence fixture player exists");

        let dispatch = session
            .dispatch_global(1, &handler, &[multi_receiver.clone()])
            .expect("external Multiuser instance should enter the owned pending route");
        assert!(matches!(
            dispatch,
            GlobalDispatch::PendingRequest {
                request: InternalVmRequest::ExternalXtra(_),
                ..
            }
        ));
        let dispatch = session
            .dispatch_global(1, &handler, &[curl_receiver])
            .expect("external Curl instance should enter the owned pending route");
        assert!(matches!(
            dispatch,
            GlobalDispatch::PendingRequest {
                request: InternalVmRequest::ExternalXtra(_),
                ..
            }
        ));
        let dispatch = session
            .dispatch_global(1, &builtin_handler, &[multi_receiver])
            .expect("builtin handler should retain precedence over external Xtra routing");
        assert!(matches!(
            dispatch,
            GlobalDispatch::PendingRequest {
                request: InternalVmRequest::MovieAsync(_),
                ..
            }
        ));
    }

    #[test]
    fn prepared_intent_is_rejected_after_close_and_id_reuse() {
        use async_std::channel;
        use crate::director::lingo::datum::Datum;
        use crate::player::symbols::symbol_table::SymbolTable;

        let owner = OwnerToken::transitional();
        let (tx, _rx) = channel::unbounded();
        let mut player = crate::player::DirPlayer::new_with_owner(tx, owner);
        player.external_params.insert("multiuser_websocket_path".to_owned(), "/owned".to_owned());
        player.external_params.insert("multiuser_websocket_ssl".to_owned(), "1".to_owned());
        let mut symbols = SymbolTable::new();
        let instance_id = player.with_xtra_manager_state(|state, player| {
            state.multiuser.create_instance_explicit(player, &[]).unwrap()
        });
        let receiver = player.alloc_datum(Datum::XtraInstance(
            "Multiuser".to_owned(),
            instance_id,
        ));
        let args = vec![
            player.alloc_datum(Datum::String("user".to_owned())),
            player.alloc_datum(Datum::String("password".to_owned())),
            player.alloc_datum(Datum::String("server.example".to_owned())),
            player.alloc_datum(Datum::Int(443)),
            player.alloc_datum(Datum::String("movie".to_owned())),
            player.alloc_datum(Datum::Int(1)),
        ];
        let intent = match call_instance_handler_pending_explicit(
            &mut player,
            &mut symbols,
            "Multiuser",
            &receiver,
            "connectToNetServer",
            &args,
        )
        .unwrap()
        {
            XtraPendingOrValue::Pending(intent) => intent,
            XtraPendingOrValue::Value(_) => panic!("connect must remain pending"),
        };
        match &intent {
            XtraPendingIntent::MultiuserConnect { request, .. } => {
                assert_eq!(request.websocket_path.as_deref(), Some("/owned"));
                assert_eq!(request.websocket_ssl, Some(true));
            }
            _ => panic!("connect did not retain its owned WebSocket configuration"),
        }
        player.with_xtra_manager_state(|state, _| {
            state.validate_pending_intent(&intent).unwrap();
        });

        call_instance_handler_pending_explicit(
            &mut player,
            &mut symbols,
            "Multiuser",
            &receiver,
            "breakconnection",
            &[],
        )
        .unwrap();
        player.with_xtra_manager_state(|state, _| {
            assert!(state.validate_pending_intent(&intent).is_err());
        });
        let reconnected = match call_instance_handler_pending_explicit(
            &mut player,
            &mut symbols,
            "Multiuser",
            &receiver,
            "connectToNetServer",
            &args,
        )
        .unwrap()
        {
            XtraPendingOrValue::Pending(intent) => intent,
            XtraPendingOrValue::Value(_) => panic!("connect must remain pending"),
        };
        player.with_xtra_manager_state(|state, _| {
            state.validate_pending_intent(&reconnected).unwrap();
        });

        call_instance_handler_pending_explicit(
            &mut player,
            &mut symbols,
            "Multiuser",
            &receiver,
            "close",
            &[],
        )
        .unwrap();
        player.with_xtra_manager_state(|state, _| {
            assert!(state.validate_pending_intent(&intent).is_err());
            state.reset();
        });
        let replacement_id = player.with_xtra_manager_state(|state, player| {
            state.multiuser.create_instance_explicit(player, &[]).unwrap()
        });
        assert_eq!(replacement_id, instance_id);
        player.with_xtra_manager_state(|state, _| {
            assert!(state.validate_pending_intent(&intent).is_err());
        });
    }

    #[test]
    fn reset_retains_exact_teardown_identity_for_owned_instances() {
        let owner = OwnerToken::transitional();
        let mut state = XtraManagerState::new(owner.clone());
        let id = state.multiuser.create_instance(&Vec::new());
        state.reset();
        let requests = state.take_teardown_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].xtra_name, "Multiuser");
        assert_eq!(requests[0].instance_id, id);
        assert_eq!(requests[0].generation, 1);
        assert!(requests[0].owner.same_identity(&owner));
    }

    #[test]
    fn session_drains_normal_close_teardown_after_borrow_release() {
        use async_std::channel;
        use crate::director::lingo::datum::Datum;
        use crate::player::session::RuntimeSession;
        use crate::player::symbols::symbol_table::SymbolOwner;
        use std::{cell::RefCell, rc::Rc};

        let session = Rc::new(RefCell::new(RuntimeSession::new(SymbolOwner {
            session: 811,
            generation: 1,
        })));
        let (tx, _rx) = channel::unbounded();
        assert!(session.borrow_mut().add_player(1, tx));
        let fired = Rc::new(std::cell::Cell::new(false));
        let weak_session = Rc::downgrade(&session);
        let probe = {
            let fired = fired.clone();
            Rc::new(move || {
                assert!(
                    weak_session
                        .upgrade()
                        .expect("session remains live")
                        .try_borrow_mut()
                        .is_ok(),
                    "teardown Drop must run outside the session RefMut"
                );
                fired.set(true);
            })
        };
        session
            .borrow_mut()
            .with_player(1, |mut context| {
                let instance_id = context
                    .player
                    .with_xtra_manager_state(|state, player| {
                        state.multiuser.create_instance_explicit(player, &[])
                    })
                    .expect("Multiuser instance creation should succeed");
                context.player.with_xtra_manager_state(|state, _| {
                    state
                        .multiuser
                        .instances
                        .get_mut(&instance_id)
                        .expect("Multiuser instance remains installed")
                        .teardown_probe = Some(probe.clone());
                });
                let receiver = context
                    .player
                    .alloc_datum(Datum::XtraInstance("Multiuser".to_owned(), instance_id));
                call_instance_handler_pending_explicit(
                    context.player,
                    context.symbols,
                    "Multiuser",
                    &receiver,
                    "close",
                    &[],
                )
                .expect("normal close should remain synchronous");
            })
            .expect("session player exists");
        session.borrow_mut().collect_player_host_teardowns(1);
        crate::player::commands::drain_host_teardowns(&session);
        assert!(fired.get(), "production drain must drop the queued resource");
    }

    #[test]
    fn session_reset_drains_active_xtra_teardown_after_borrow_release() {
        use async_std::channel;
        use crate::player::session::RuntimeSession;
        use crate::player::symbols::symbol_table::SymbolOwner;
        use std::{cell::RefCell, rc::Rc};

        let session = Rc::new(RefCell::new(RuntimeSession::new(SymbolOwner {
            session: 813,
            generation: 1,
        })));
        let (tx, _rx) = channel::unbounded();
        assert!(session.borrow_mut().add_player(1, tx));
        let owner = session
            .borrow_mut()
            .with_player(1, |context| context.player.owner.clone())
            .expect("player exists");
        let fired = Rc::new(std::cell::Cell::new(false));
        let weak_session = Rc::downgrade(&session);
        let probe = {
            let fired = fired.clone();
            Rc::new(move || {
                assert!(
                    weak_session
                        .upgrade()
                        .expect("session remains live")
                        .try_borrow_mut()
                        .is_ok(),
                    "reset teardown Drop must run outside the session RefMut"
                );
                fired.set(true);
            })
        };
        session
            .borrow_mut()
            .with_player(1, |context| {
                context.player.with_xtra_manager_state(|state, player| {
                    let instance_id = state
                        .multiuser
                        .create_instance_explicit(player, &[])
                        .expect("Multiuser instance creation should succeed");
                    state
                        .multiuser
                        .instances
                        .get_mut(&instance_id)
                        .expect("Multiuser instance remains installed")
                        .teardown_probe = Some(probe.clone());
                });
            })
            .expect("player exists");
        let replacement = session
            .borrow_mut()
            .reset_player_owned(1, &owner)
            .expect("captured owner reset should succeed");
        assert!(!replacement.same_identity(&owner));
        assert_eq!(session.borrow().pending_host_teardown_count(), 1);
        crate::player::commands::drain_host_teardowns(&session);
        assert!(fired.get(), "production drain must drop reset resources");
    }

    #[test]
    fn remove_moves_active_xtra_resources_before_player_drop() {
        use async_std::channel;
        use crate::player::session::RuntimeSession;
        use crate::player::symbols::symbol_table::SymbolOwner;
        use std::{cell::RefCell, rc::Rc};

        let session = Rc::new(RefCell::new(RuntimeSession::new(SymbolOwner {
            session: 812,
            generation: 1,
        })));
        let (tx_a, _rx_a) = channel::unbounded();
        let (tx_b, _rx_b) = channel::unbounded();
        assert!(session.borrow_mut().add_player(1, tx_a));
        assert!(session.borrow_mut().add_player(2, tx_b));
        let fired = Rc::new(std::cell::Cell::new(false));
        let weak_session = Rc::downgrade(&session);
        let probe = {
            let fired = fired.clone();
            Rc::new(move || {
                assert!(
                    weak_session
                        .upgrade()
                        .expect("session remains live")
                        .try_borrow_mut()
                        .is_ok(),
                    "active resource teardown must run outside the session RefMut"
                );
                fired.set(true);
            })
        };
        session
            .borrow_mut()
            .with_player(1, |context| {
                context.player.with_xtra_manager_state(|state, player| {
                    let instance_id = state
                        .multiuser
                        .create_instance_explicit(player, &[])
                        .expect("Multiuser instance creation should succeed");
                    state
                        .multiuser
                        .instances
                        .get_mut(&instance_id)
                        .expect("Multiuser instance remains installed")
                        .teardown_probe = Some(probe.clone());
                });
            })
            .expect("first player exists");
        let owner_two = session
            .borrow_mut()
            .with_player(2, |context| context.player.owner.clone())
            .expect("second player exists");
        {
            let mut borrowed = session.borrow_mut();
            assert!(borrowed.remove_player(1).is_some());
            assert_eq!(
                borrowed.pending_host_teardown_count(),
                1,
                "active Xtra must be queued before removal"
            );
        }
        crate::player::commands::drain_host_teardowns(&session);
        assert!(fired.get(), "production drain must drop the removed resource");
        assert!(session
            .borrow_mut()
            .with_player(2, |context| context.player.owner.same_identity(&owner_two))
            .unwrap_or(false));
    }

    #[test]
    fn fileio_virtual_files_survive_instance_reset_but_instances_do_not() {
        let owner = OwnerToken::transitional();
        let mut state = XtraManagerState::new(owner.clone());
        state.fileio.virtual_fs.insert("remote.txt".to_owned(), b"owned".to_vec());
        let id = state.fileio.create_instance_explicit(&[]).unwrap();
        assert_eq!(state.fileio.instance_generation(id), Some(1));
        state.fileio.reset();
        assert!(state.fileio.instance_generation(id).is_none());
        assert_eq!(state.fileio.virtual_fs.get("remote.txt"), Some(&b"owned".to_vec()));
    }

    #[test]
    fn fileio_open_request_is_rejected_after_reset_and_id_reuse() {
        let owner = OwnerToken::transitional();
        let mut state = XtraManagerState::new(owner.clone());
        let id = state.fileio.create_instance_explicit(&[]).unwrap();
        let generation = state.fileio.instance_generation(id).unwrap();
        let intent = XtraPendingIntent::FileIoOpen(FileIoOpenRequest {
            owner: owner.clone(),
            instance_id: id,
            generation,
            file_name: "remote.txt".to_owned(),
            relative_name: "remote.txt".to_owned(),
            prepared: crate::player::net_manager::PreparedNetTask {
                task: crate::player::net_task::NetTask::new(
                    1,
                    "remote.txt",
                    &url::Url::parse("https://example.invalid/remote.txt").unwrap(),
                ),
                owner_key: owner.key(),
                shared_state: std::sync::Arc::new(async_std::sync::Mutex::new(
                    crate::player::net_manager::NetManagerSharedState::new(),
                )),
                should_start: false,
            },
            mode: 1,
        });
        state.validate_pending_intent(&intent).unwrap();
        state.reset();
        state.rebind_owner(owner.clone());
        let replacement = state.fileio.create_instance_explicit(&[]).unwrap();
        assert_eq!(replacement, id);
        assert!(state.validate_pending_intent(&intent).is_err());
    }

    #[test]
    fn fileio_missing_open_prepares_owned_request_for_all_modes() {
        use async_std::channel;
        let owner = OwnerToken::transitional();
        let (tx, _rx) = channel::unbounded();
        let mut player = crate::player::DirPlayer::new_with_owner(tx, owner.clone());
        player.net_manager.set_base_path(url::Url::parse("https://example.invalid/").unwrap());
        let mut symbols = crate::player::symbols::symbol_table::SymbolTable::new();
        let instance_id = player
            .with_xtra_manager_state(|state, _| state.fileio.create_instance_explicit(&[]))
            .unwrap();
        let receiver = player.alloc_datum(Datum::XtraInstance("FileIO".to_owned(), instance_id));
        let file_name = player.alloc_datum(Datum::String("remote.txt".to_owned()));
        let mode = player.alloc_datum(Datum::Int(0));
        let pending = call_instance_handler_pending_explicit(
            &mut player,
            &mut symbols,
            "FileIO",
            &receiver,
            "openFile",
            &[file_name, mode],
        )
        .unwrap();
        match pending {
            XtraPendingOrValue::Pending(XtraPendingIntent::FileIoOpen(request)) => {
                assert_eq!(request.file_name, "remote.txt");
                assert_eq!(request.relative_name, "remote.txt");
                assert_eq!(
                    request.prepared.task.resolved_url.to_string(),
                    "https://example.invalid/remote.txt"
                );
                assert_eq!(request.mode, 0);
                assert!(request.owner.same_identity(&owner));
            }
            _ => panic!("missing non-virtual FileIO open must be typed pending work"),
        }
    }

    #[test]
    fn fileio_open_snapshots_resolved_url_before_network_config_changes() {
        use async_std::channel;
        use url::Url;

        let owner = OwnerToken::transitional();
        let (tx, _rx) = channel::unbounded();
        let mut player = crate::player::DirPlayer::new_with_owner(tx, owner.clone());
        player.net_manager.set_base_path(Url::parse("https://first.example/").unwrap());
        let mut symbols = crate::player::symbols::symbol_table::SymbolTable::new();
        let instance_id = player
            .with_xtra_manager_state(|state, _| state.fileio.create_instance_explicit(&[]))
            .unwrap();
        let receiver = player.alloc_datum(Datum::XtraInstance("FileIO".to_owned(), instance_id));
        let file_name = player.alloc_datum(Datum::String("remote.txt".to_owned()));
        let pending = call_instance_handler_pending_explicit(
            &mut player,
            &mut symbols,
            "FileIO",
            &receiver,
            "openFile",
            &[file_name],
        )
        .unwrap();
        let request = match pending {
            XtraPendingOrValue::Pending(XtraPendingIntent::FileIoOpen(request)) => request,
            _ => panic!("missing remote open request"),
        };
        assert_eq!(request.prepared.task.resolved_url.to_string(), "https://first.example/remote.txt");
        player.net_manager.set_base_path(Url::parse("https://second.example/").unwrap());
        assert_eq!(
            player.net_manager.get_task(request.prepared.task.id).unwrap().resolved_url.to_string(),
            request.prepared.task.resolved_url.to_string()
        );
    }

    #[test]
    fn fileio_remote_open_error_preserves_legacy_mode_codes() {
        assert_eq!(crate::player::xtra::fileio::remote_open_error(1), -43);
        assert_eq!(crate::player::xtra::fileio::remote_open_error(0), 0);
        assert_eq!(crate::player::xtra::fileio::remote_open_error(2), 0);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn fileio_old_task_completion_cannot_satisfy_reused_task_id() {
        use async_std::channel;
        use url::Url;

        let owner = OwnerToken::transitional();
        let (tx, _rx) = channel::unbounded();
        let mut player = crate::player::DirPlayer::new_with_owner(tx, owner.clone());
        player.net_manager.set_base_path(Url::parse("https://old.example/").unwrap());
        let old = player.net_manager.prepare_net_thing("same.txt".to_owned());
        let old_shared = old.shared_state.clone();
        let old_task_id = old.task.id;
        player.net_manager.reset_owner(owner.key().next_generation());
        player.net_manager.set_base_path(Url::parse("https://new.example/").unwrap());
        let new = player.net_manager.prepare_net_thing("same.txt".to_owned());
        assert_eq!(old.task.id, new.task.id, "reset deliberately reuses task ids");
        async_std::task::block_on(async {
            old_shared
                .lock()
                .await
                .fulfill_task(old_task_id, Ok(b"old".to_vec()))
                .await;
        });
        assert!(player.net_manager.get_task_result(Some(new.task.id)).is_none());
        assert_eq!(new.task.resolved_url.to_string(), "https://new.example/same.txt");
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn execute_failed_fileio_open(mode: i32) -> i32 {
        use async_std::channel;
        use crate::player::session::RuntimeSession;
        use crate::player::symbols::symbol_table::SymbolOwner;
        use std::{cell::RefCell, rc::Rc};

        let session = Rc::new(RefCell::new(RuntimeSession::new(SymbolOwner {
            session: 933,
            generation: 1,
        })));
        let (tx, _rx) = channel::unbounded();
        assert!(session.borrow_mut().add_player(1, tx));
        let (intent, instance_id, shared_state) = session
            .borrow_mut()
            .with_player(1, |mut context| {
                context
                    .player
                    .net_manager
                    .set_base_path(url::Url::parse("https://example.invalid/").unwrap());
                let instance_id = context
                    .player
                    .with_xtra_manager_state(|state, _| state.fileio.create_instance_explicit(&[]))
                    .unwrap();
                let receiver = context.player.alloc_datum(Datum::XtraInstance("FileIO".to_owned(), instance_id));
                let file_name = context.player.alloc_datum(Datum::String("missing.txt".to_owned()));
                let mode_ref = context.player.alloc_datum(Datum::Int(mode));
                let pending = call_instance_handler_pending_explicit(
                    context.player,
                    context.symbols,
                    "FileIO",
                    &receiver,
                    "openFile",
                    &[file_name, mode_ref],
                )
                .unwrap();
                let intent = match pending {
                    XtraPendingOrValue::Pending(intent) => intent,
                    XtraPendingOrValue::Value(_) => panic!("missing FileIO open must suspend"),
                };
                let shared_state = context.player.net_manager.shared_state.clone();
                (intent, instance_id, shared_state)
            })
            .unwrap();
        let task_id = match &intent {
            XtraPendingIntent::FileIoOpen(request) => request.prepared.task.id,
            _ => panic!("wrong pending intent for FileIO open"),
        };
        async_std::task::block_on(async {
            shared_state
                .lock()
                .await
                .fulfill_task(task_id, Err(7))
                .await;
        });
        async_std::task::block_on(Box::pin(execute_pending_intent(&session, 1, intent)))
            .expect("completed FileIO failure should still open the instance");
        let (is_open, last_error) = session
            .borrow_mut()
            .with_player(1, |context| {
                let instance = &context.player.xtra_manager_state.fileio.instances[&instance_id];
                (instance.is_open, instance.last_error)
            })
            .unwrap();
        assert!(is_open, "completed FileIO failure must still open the instance");
        last_error
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn fileio_failed_open_uses_production_completion_for_each_mode() {
        assert_eq!(execute_failed_fileio_open(1), -43);
        assert_eq!(execute_failed_fileio_open(0), 0);
        assert_eq!(execute_failed_fileio_open(2), 0);
    }

    #[test]
    fn budapi_state_is_owner_local_and_foreign_args_are_rejected() {
        use async_std::channel;
        use std::{cell::RefCell, rc::Rc};
        use crate::director::lingo::datum::Datum;
        use crate::player::session::RuntimeSession;
        use crate::player::symbols::symbol_table::SymbolOwner;

        let session = Rc::new(RefCell::new(RuntimeSession::new(SymbolOwner {
            session: 915,
            generation: 1,
        })));
        let (tx_a, _rx_a) = channel::unbounded();
        let (tx_b, _rx_b) = channel::unbounded();
        assert!(session.borrow_mut().add_player(1, tx_a));
        assert!(session.borrow_mut().add_player(2, tx_b));

        session.borrow_mut().with_player(1, |context| {
            let receiver = context.player.alloc_datum(Datum::Void);
            let value = context.player.alloc_datum(Datum::Int(37));
            let symbols = &*context.symbols;
            context.player.with_xtra_manager_state(|state, player| {
                BudApiXtra::call_handler(
                    player,
                    &mut state.budapi,
                    "baSetVolume",
                    &vec![receiver, value],
                    symbols,
                )
            }).unwrap();
        });
        session.borrow_mut().with_player(2, |context| {
            let receiver = context.player.alloc_datum(Datum::Void);
            let value = context.player.alloc_datum(Datum::Int(91));
            let symbols = &*context.symbols;
            context.player.with_xtra_manager_state(|state, player| {
                BudApiXtra::call_handler(
                    player,
                    &mut state.budapi,
                    "baSetVolume",
                    &vec![receiver, value],
                    symbols,
                )
            })
        }).unwrap();
        let volume_a = session
            .borrow_mut()
            .with_player(1, |context| context.player.xtra_manager_state.budapi.sound_volume)
            .unwrap();
        let volume_b = session
            .borrow_mut()
            .with_player(2, |context| context.player.xtra_manager_state.budapi.sound_volume)
            .unwrap();
        let volumes = (volume_a, volume_b);
        assert_eq!(volumes, (37, 91));

        let owner_a = session
            .borrow_mut()
            .with_player(1, |context| context.player.owner.clone())
            .unwrap();
        session
            .borrow_mut()
            .reset_player_owned(1, &owner_a)
            .expect("owner A reset should succeed");
        let reset_volume_a = session
            .borrow_mut()
            .with_player(1, |context| context.player.xtra_manager_state.budapi.sound_volume)
            .unwrap();
        let reset_volume_b = session
            .borrow_mut()
            .with_player(2, |context| context.player.xtra_manager_state.budapi.sound_volume)
            .unwrap();
        let reset_volumes = (reset_volume_a, reset_volume_b);
        assert_eq!(reset_volumes, (100, 91), "reset must clear only owner A BudAPI state");

        let owner_a_after_reset = session.borrow_mut().with_player(1, |context| {
            context.player.xtra_manager_state.budapi.clipboard_text = "A clipboard".to_owned();
            context.player.owner.clone()
        }).unwrap();
        let owner_b = session.borrow_mut().with_player(2, |context| {
            context.player.xtra_manager_state.budapi.clipboard_text = "B clipboard".to_owned();
            context.player.owner.clone()
        }).unwrap();
        let read_clipboard = |player_id, owner: OwnerToken| {
            let value = execute_budapi_clipboard_read(
                &session,
                player_id,
                BudApiHostIntent::ClipboardRead { owner },
            ).expect("owner-local clipboard read should succeed");
            session.borrow_mut().with_player(player_id, |context| {
                match context.player.allocator.try_get_datum(&value) {
                    Some(Datum::String(text)) => text.clone(),
                    _ => panic!("unexpected clipboard datum"),
                }
            }).unwrap()
        };
        assert_eq!(read_clipboard(1, owner_a_after_reset.clone()), "A clipboard");
        assert_eq!(read_clipboard(2, owner_b), "B clipboard");
        session.borrow_mut().reset_player_owned(1, &owner_a_after_reset)
            .expect("owner A reset should succeed");
        let owner_a_replacement = session.borrow_mut().with_player(1, |context| context.player.owner.clone()).unwrap();
        assert_eq!(read_clipboard(1, owner_a_replacement), "");
        let owner_b_replacement = session.borrow_mut().with_player(2, |context| context.player.owner.clone()).unwrap();
        assert_eq!(read_clipboard(2, owner_b_replacement), "B clipboard");

        let foreign_arg = session
            .borrow_mut()
            .with_player(1, |context| context.player.alloc_datum(Datum::String("foreign".to_owned())))
            .unwrap();
        let rejected = session.borrow_mut().with_player(2, |context| {
            prepare_openurl_handler_explicit(context.player, context.symbols, "gsOpenURL", &[foreign_arg])
                .expect("OpenURL handler should be recognized")
        });
        assert!(matches!(rejected, Some(Err(error)) if error.code == crate::player::ScriptErrorCode::InvalidReference));
    }

    #[test]
    fn budapi_host_intent_reentry_is_borrow_free_and_reset_rejects_late_result() {
        use async_std::channel;
        use std::{cell::{Cell, RefCell}, rc::Rc};
        use crate::director::lingo::datum::Datum;
        use crate::player::session::RuntimeSession;
        use crate::player::symbols::symbol_table::SymbolOwner;

        let session = Rc::new(RefCell::new(RuntimeSession::new(SymbolOwner {
            session: 916,
            generation: 1,
        })));
        let (tx, _rx) = channel::unbounded();
        assert!(session.borrow_mut().add_player(1, tx));
        let (intent, owner) = session
            .borrow_mut()
            .with_player(1, |context| {
                let message = context.player.alloc_datum(Datum::String("owned".to_owned()));
                let outcome = prepare_budapi_handler_explicit(
                    context.player,
                    context.symbols,
                    "baMsgBox",
                    &[message],
                )
                .expect("BudAPI handler should be recognized")
                .expect("BudAPI message box should prepare");
                let intent = match outcome {
                    XtraPendingOrValue::Pending(XtraPendingIntent::BudApi(intent)) => intent,
                    _ => panic!("message box did not produce a typed BudAPI intent"),
                };
                (intent, context.player.owner.clone())
            })
            .unwrap();
        let reentered = Rc::new(Cell::new(false));
        let reentered_for_host = reentered.clone();
        let result = execute_budapi_host_intent_with(&session, 1, intent, |request| {
            assert!(session.try_borrow_mut().is_ok());
            reentered_for_host.set(true);
            session
                .borrow_mut()
                .reset_player_owned(1, &owner)
                .expect("host reset should succeed");
            let _ = request;
            true
        });
        assert!(reentered.get());
        assert!(result.is_err(), "completion after owner reset must be rejected");
    }

    #[test]
    fn budapi_host_intent_cannot_cross_players() {
        use async_std::channel;
        use std::{cell::{Cell, RefCell}, rc::Rc};
        use crate::director::lingo::datum::Datum;
        use crate::player::session::RuntimeSession;
        use crate::player::symbols::symbol_table::SymbolOwner;

        let session = Rc::new(RefCell::new(RuntimeSession::new(SymbolOwner {
            session: 917,
            generation: 1,
        })));
        let (tx_a, _rx_a) = channel::unbounded();
        let (tx_b, _rx_b) = channel::unbounded();
        assert!(session.borrow_mut().add_player(1, tx_a));
        assert!(session.borrow_mut().add_player(2, tx_b));
        let intent = session
            .borrow_mut()
            .with_player(1, |context| {
                let message = context.player.alloc_datum(Datum::String("cross-owner".to_owned()));
                match prepare_budapi_handler_explicit(context.player, context.symbols, "baMsgBox", &[message])
                    .expect("BudAPI handler should be recognized")
                    .expect("BudAPI message box should prepare")
                {
                    XtraPendingOrValue::Pending(XtraPendingIntent::BudApi(intent)) => intent,
                    _ => panic!("message box did not produce a typed BudAPI intent"),
                }
            })
            .unwrap();
        let invoked = Rc::new(Cell::new(false));
        let invoked_by_host = invoked.clone();
        let result = execute_budapi_host_intent_with(&session, 2, intent, |_request| {
            invoked_by_host.set(true);
            true
        });
        assert!(result.is_err(), "foreign player must reject the host intent");
        assert!(!invoked.get(), "foreign host intent must not invoke the browser");
    }
}

pub fn call_xtra_instance_handler(
    player: &mut crate::player::DirPlayer,
    xtra_name: &str,
    instance_id: XtraInstanceId,
    handler_name: &str,
    args: &Vec<DatumRef>,
    symbols: &mut crate::player::symbols::symbol_table::SymbolTable,
) -> Result<DatumRef, ScriptError> {
    let xtra_name_lower = xtra_name.to_lowercase();
    match xtra_name_lower.as_str() {
        "xmlparser" => {
            let receiver = player.alloc_datum(Datum::XtraInstance(xtra_name.to_owned(), instance_id));
            return call_instance_handler_explicit(player, symbols, xtra_name, &receiver, handler_name, args)
        }
        "fileio" => {
            let receiver = player.alloc_datum(Datum::XtraInstance(xtra_name.to_owned(), instance_id));
            return call_instance_handler_explicit(player, symbols, xtra_name, &receiver, handler_name, args)
        }
        // Static-only, but stateless: its handlers write to player-level
        // overrides, so an instance-syntax call (`lprh.setTheMoviePath(…)`)
        // means exactly the same thing as the bare global the README shows.
        "leechprotectionremovalhelp" if LeechProtectionXtra::has_handler(handler_name) => {
            return LeechProtectionXtra::call_handler_explicit(player, handler_name, args, symbols)
        }
        _ => Err(ScriptError::new(format!(
            "No handler {} found for xtra {} instance #{}",
            handler_name, xtra_name, instance_id
        ))),
    }
}
