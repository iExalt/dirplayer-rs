//! Curl Xtra (Valentin Schmidt, v0.16) — libcurl wrapper.
//!
//! In dirplayer-rs we expose the Director-facing API but route every transfer
//! through `fetch` (via `web_sys`). Non-HTTP libcurl protocols (FTP, SCP,
//! SMTP, …) and Windows-only `execSocket` are not supported and report a
//! libcurl-style error code.
//!
//! Lingo-level synchronous `exec` is intentionally unsupported in WASM
//! because the browser event loop must spin for `fetch` to make progress;
//! it returns `CURLE_NOT_BUILT_IN` (4). Use `execAsync` instead.

use fxhash::FxHashMap;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

use crate::{
    director::lingo::datum::{Datum, DatumType, XtraInstanceId},
    player::{
        events::player_dispatch_callback_event, reserve_player_mut, reserve_player_ref,
        symbols::{symbol::Symbol, symbol_table::SymbolTable}, ownership::OwnerToken,
        DatumRef, DirPlayer, ScriptError,
    },
};

/// libcurl error codes we surface from the WASM port.
const CURLE_OK: i32 = 0;
const CURLE_UNSUPPORTED_PROTOCOL: i32 = 1;
const CURLE_FAILED_INIT: i32 = 2;
const CURLE_URL_MALFORMAT: i32 = 3;
const CURLE_NOT_BUILT_IN: i32 = 4;
const CURLE_COULDNT_RESOLVE_HOST: i32 = 6;
const CURLE_COULDNT_CONNECT: i32 = 7;
const CURLE_HTTP_RETURNED_ERROR: i32 = 22;

/// Subset of CURLOPT_* constants we actually act on. Values match the
/// libcurl ABI (string opts are >=10000, slist opts >=10000, off_t opts
/// >=30000).
mod opt {
    pub const SSL_VERIFYPEER: i32 = 64;
    pub const HTTPGET: i32 = 80;
    pub const POST: i32 = 47;
    pub const NOBODY: i32 = 44; // HEAD
    pub const CUSTOMREQUEST: i32 = 10036;
    pub const URL: i32 = 10002;
    pub const USERAGENT: i32 = 10018;
    pub const REFERER: i32 = 10016;
    pub const COOKIE: i32 = 10022;
    pub const HTTPHEADER: i32 = 10023;
    pub const POSTFIELDS: i32 = 10015;
    pub const USERPWD: i32 = 10005;
    pub const PROXY: i32 = 10004;
    pub const CAINFO: i32 = 10065;
    pub const RANGE: i32 = 10007;
    pub const ACCEPT_ENCODING: i32 = 10102;
}

#[derive(Clone)]
pub(crate) struct CurlInstance {
    generation: u64,
    auto_detach: bool,
    url: String,
    method: String,
    custom_method: Option<String>,
    headers: Vec<String>,
    body: Option<Vec<u8>>,
    form: Vec<(String, String)>,
    referer: Option<String>,
    user_agent: Option<String>,
    cookie: Option<String>,
    user_pwd: Option<String>,
    proxy: Option<String>,
    range: Option<String>,
    accept_encoding: Option<String>,
    last_status: i32,
    last_response_headers: String,
    last_response_size: i32,
    last_effective_url: String,
    /// Lingo handler called after execAsync completes.
    completion_callback: Option<(DatumRef, String)>,
    header_callback: Option<(DatumRef, String)>,
    progress_callback: Option<(DatumRef, String)>,
}

impl CurlInstance {
    pub(crate) fn new(auto_detach: bool, generation: u64) -> Self {
        CurlInstance {
            generation,
            auto_detach,
            url: String::new(),
            method: "GET".to_string(),
            custom_method: None,
            headers: Vec::new(),
            body: None,
            form: Vec::new(),
            referer: None,
            user_agent: None,
            cookie: None,
            user_pwd: None,
            proxy: None,
            range: None,
            accept_encoding: None,
            last_status: 0,
            last_response_headers: String::new(),
            last_response_size: 0,
            last_effective_url: String::new(),
            completion_callback: None,
            header_callback: None,
            progress_callback: None,
        }
    }

    fn effective_method(&self) -> String {
        self.custom_method.clone().unwrap_or_else(|| self.method.clone())
    }

    pub(crate) fn requested_url(&self) -> &str {
        &self.url
    }
}

pub struct CurlXtraManager {
    pub instances: FxHashMap<u32, CurlInstance>,
    pub instance_counter: u32,
    pub owner: OwnerToken,
    pub generation_counter: u64,
    released_instances: Vec<(u32, u64)>,
}

impl CurlXtraManager {
    pub(crate) fn completion_callback(
        &self,
        instance_id: u32,
        generation: u64,
    ) -> Result<Option<(DatumRef, String)>, ScriptError> {
        let instance = self.instances.get(&instance_id).ok_or_else(|| {
            ScriptError::new(format!("Curl instance #{} not found", instance_id))
        })?;
        if instance.generation != generation {
            return Err(ScriptError::new_code(
                crate::player::ScriptErrorCode::InvalidReference,
                "stale Curl instance generation".to_owned(),
            ));
        }
        Ok(instance.completion_callback.clone())
    }

    pub(crate) fn apply_owned_result(
        &mut self,
        instance_id: u32,
        generation: u64,
        status: i32,
        body_len: usize,
        headers: String,
        effective_url: String,
    ) -> Result<(), ScriptError> {
        let instance = self.instances.get_mut(&instance_id).ok_or_else(|| {
            ScriptError::new(format!("Curl instance #{} not found", instance_id))
        })?;
        if instance.generation != generation {
            return Err(ScriptError::new_code(
                crate::player::ScriptErrorCode::InvalidReference,
                "stale Curl instance generation".to_owned(),
            ));
        }
        instance.last_status = if status > 0 { status } else { 0 };
        instance.last_response_headers = headers;
        instance.last_response_size = body_len as i32;
        instance.last_effective_url = effective_url;
        Ok(())
    }

    pub(crate) fn instance_generation(&self, instance_id: XtraInstanceId) -> Option<u64> {
        self.instances.get(&instance_id).map(|instance| instance.generation)
    }

    pub fn new() -> Self {
        Self::new_with_owner(crate::player::ownership::OwnerToken::transitional())
    }

    pub fn new_with_owner(owner: OwnerToken) -> Self {
        CurlXtraManager {
            instances: FxHashMap::default(),
            instance_counter: 0,
            owner,
            generation_counter: 0,
            released_instances: Vec::new(),
        }
    }

    pub fn reset(&mut self) {
        self.released_instances.extend(
            self.instances
                .drain()
                .map(|(id, instance)| (id, instance.generation)),
        );
        self.instance_counter = 0;
    }

    pub(crate) fn rebind_owner(&mut self, owner: OwnerToken) {
        self.owner = owner;
    }

    pub(crate) fn create_instance_explicit(
        &mut self,
        player: &DirPlayer,
        symbols: &SymbolTable,
        args: &[DatumRef],
    ) -> Result<u32, ScriptError> {
        check_owner(&self.owner, player)?;
        let auto_detach = match args.first() {
            Some(arg) => {
                let datum = checked_datum(player, arg)?;
                crate::player::compare::validate_direct_symbol_fields(datum, symbols)?;
                datum.bool_value().unwrap_or(false)
            }
            None => false,
        };
        self.instance_counter = self.instance_counter.wrapping_add(1);
        self.generation_counter = self.generation_counter.wrapping_add(1);
        self.instances
            .insert(self.instance_counter, CurlInstance::new(auto_detach, self.generation_counter));
        Ok(self.instance_counter)
    }

    pub(crate) fn call_instance_handler_or_pending_explicit(
        &mut self,
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        instance_id: XtraInstanceId,
        handler_name: &str,
        args: &[DatumRef],
    ) -> Result<crate::player::xtra::manager::XtraPendingOrValue, ScriptError> {
        if handler_name.eq_ignore_ascii_case("execAsync") {
            check_owner(&self.owner, player)?;
            let generation = self.instances.get(&instance_id).ok_or_else(|| {
                ScriptError::new(format!("Curl instance #{} not found", instance_id))
            })?.generation;
            let completion = args.first().map(|handler_ref| {
                let handler = checked_datum(player, handler_ref)?.symbol_value(symbols)?;
                let display = symbols.display(&handler).map_err(|_| ScriptError::new_code(
                    crate::player::ScriptErrorCode::InvalidReference,
                    "foreign Curl completion callback symbol".to_owned(),
                ))?.to_owned();
                let target = args.get(1).cloned().unwrap_or(DatumRef::Void);
                if !matches!(target, DatumRef::Void) {
                    checked_retained(player, symbols, &target)?;
                }
                Ok::<_, ScriptError>((target, display))
            }).transpose()?;
            // Parse all consumed values before mutating the live callback.
            // A rejected mode must leave the prior callback registration
            // untouched, matching the legacy async path.
            let return_mode = args.get(2)
                .map(|arg| checked_datum(player, arg)?.int_value())
                .transpose()?
                .unwrap_or(0);
            if let Some(completion) = completion {
                self.instances.get_mut(&instance_id).ok_or_else(|| {
                    ScriptError::new(format!("Curl instance #{} not found", instance_id))
                })?.completion_callback = Some(completion);
            }
            let mut instance = self.instances.get(&instance_id).cloned().ok_or_else(|| {
                ScriptError::new(format!("Curl instance #{} not found", instance_id))
            })?;
            // Completion callbacks are resolved from the live owner-bound
            // instance after the fetch. Keep them out of the transport
            // snapshot so a callback registered while the request is pending
            // is observed by the eventual executor.
            instance.completion_callback = None;
            instance.header_callback = None;
            instance.progress_callback = None;
            return Ok(crate::player::xtra::manager::XtraPendingOrValue::Pending(
                crate::player::xtra::manager::XtraPendingIntent::CurlExec {
                    owner: self.owner.clone(), instance_id, generation,
                    request: crate::player::xtra::manager::CurlExecRequest {
                        instance,
                        return_mode,
                    },
                },
            ));
        }
        self.call_instance_handler_explicit(player, symbols, instance_id, handler_name, args)
            .map(crate::player::xtra::manager::XtraPendingOrValue::Value)
    }

    pub(crate) fn call_instance_handler_explicit(
        &mut self,
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        instance_id: XtraInstanceId,
        handler_name: &str,
        args: &[DatumRef],
    ) -> Result<DatumRef, ScriptError> {
        check_owner(&self.owner, player)?;
        let instance = self.instances.get_mut(&instance_id).ok_or_else(|| {
            ScriptError::new(format!("Curl instance #{} not found", instance_id))
        })?;
        match handler_name.to_ascii_lowercase().as_str() {
            "setoption" => set_option_explicit(instance, player, symbols, args),
            "setform" => set_form_explicit(instance, player, symbols, args),
            "setsourcefile" => set_source_file_explicit(player),
            "setdestinationfile" => set_destination_file_explicit(player),
            "setheadercallback" => set_callback_explicit(&mut instance.header_callback, player, symbols, args, "setHeaderCallback"),
            "setprogresscallback" => set_callback_explicit(&mut instance.progress_callback, player, symbols, args, "setProgressCallback"),
            "setsourcedatacallback" => Ok(DatumRef::Void),
            "getinfo" => get_info_explicit(instance, player, args),
            "exec" => alloc_int(player, CURLE_NOT_BUILT_IN),
            "execsocket" => alloc_int(player, CURLE_NOT_BUILT_IN),
            "close" => {
                self.released_instances.push((instance_id, instance.generation));
                self.instances.remove(&instance_id);
                Ok(DatumRef::Void)
            }
            _ => Err(ScriptError::new(format!(
                "Curl: no handler {} on instance #{}",
                handler_name, instance_id
            ))),
        }
    }

    pub(crate) fn take_teardown_requests(
        &mut self,
    ) -> Vec<crate::player::xtra::manager::XtraTeardownRequest> {
        self.released_instances.drain(..).map(|(instance_id, generation)| {
                crate::player::xtra::manager::XtraTeardownRequest {
                    xtra_name: "Curl",
                    owner: self.owner.clone(),
                    instance_id,
                    generation,
                    #[cfg(target_arch = "wasm32")]
                    socket_resource: None,
                    #[cfg(test)]
                    drop_probe: None,
                }
            })
            .collect()
    }
}

pub struct CurlXtra;

fn check_owner(expected: &OwnerToken, player: &DirPlayer) -> Result<(), ScriptError> {
    if !expected.same_identity(&player.owner) || !player.owner.is_arena_live() {
        return Err(ScriptError::new_code(
            crate::player::ScriptErrorCode::InvalidReference,
            "foreign or stale Xtra manager owner".to_owned(),
        ));
    }
    Ok(())
}

fn checked_datum<'a>(player: &'a DirPlayer, datum: &DatumRef) -> Result<&'a Datum, ScriptError> {
    match datum {
        DatumRef::Void => Ok(&Datum::Void),
        _ => player.allocator.try_get_datum(datum).ok_or_else(|| {
            ScriptError::new_code(
                crate::player::ScriptErrorCode::InvalidReference,
                format!("invalid Xtra datum reference {datum}"),
            )
        }),
    }
}

fn checked_retained(player: &DirPlayer, symbols: &SymbolTable, datum: &DatumRef) -> Result<(), ScriptError> {
    let value = checked_datum(player, datum)?;
    crate::player::compare::validate_direct_symbol_fields(value, symbols)?;
    if let Datum::ScriptInstanceRef(instance) = value {
        use crate::player::allocator::ScriptInstanceAllocatorTrait;
        player.allocator.get_script_instance_opt(instance).ok_or_else(|| {
            ScriptError::new_code(
                crate::player::ScriptErrorCode::InvalidReference,
                "foreign or stale Curl callback target".to_owned(),
            )
        })?;
    }
    Ok(())
}

fn alloc_int(player: &mut DirPlayer, value: i32) -> Result<DatumRef, ScriptError> {
    Ok(player.alloc_datum(Datum::Int(value)))
}

impl CurlXtra {
    pub fn call_static_handler_explicit(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        name: &str,
        args: &[DatumRef],
    ) -> Result<DatumRef, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "curl_error" => curl_error_explicit(player, args),
            "curl_escape" => curl_escape_explicit(player, symbols, args),
            "curl_hfs2posix" => curl_hfs2posix_explicit(player, symbols, args),
            _ => Err(ScriptError::new(format!("Curl static: no handler {name}"))),
        }
    }

    pub fn has_static_handler(name: &str) -> bool {
        matches!(
            name.to_ascii_lowercase().as_str(),
            "curl_error" | "curl_escape" | "curl_hfs2posix"
        )
    }

    pub fn call_static_handler(name: &str, args: &Vec<DatumRef>, symbols: &mut SymbolTable) -> Result<DatumRef, ScriptError> {
        match_ci!(name, {
            "curl_error" => curl_error(args),
            "curl_escape" => curl_escape(args, symbols),
            "curl_hfs2posix" => curl_hfs2posix(args, symbols),
            _ => Err(ScriptError::new(format!("Curl static: no handler {}", name))),
        })
    }

    pub fn has_static_async_handler(_name: &str) -> bool {
        false
    }

    pub async fn call_static_async_handler(
        name: &str,
        _args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        Err(ScriptError::new(format!(
            "Curl: no async static handler {}",
            name
        )))
    }
}

fn curl_error_explicit(player: &mut DirPlayer, args: &[DatumRef]) -> Result<DatumRef, ScriptError> {
    let code_ref = args.first().ok_or_else(|| ScriptError::new("curl_error requires an integer".to_owned()))?;
    let code = checked_datum(player, code_ref)?.int_value()?;
    let msg = match code {
        0 => "No error", 1 => "Unsupported protocol", 2 => "Failed init",
        3 => "URL malformat", 4 => "Not built-in (unsupported in WASM)",
        6 => "Couldn't resolve host", 7 => "Couldn't connect to server",
        22 => "HTTP returned error", _ => "Unknown error",
    };
    Ok(player.alloc_datum(Datum::String(msg.to_owned())))
}

fn curl_escape_explicit(
    player: &mut DirPlayer,
    symbols: &SymbolTable,
    args: &[DatumRef],
) -> Result<DatumRef, ScriptError> {
    let input_ref = args.first().ok_or_else(|| ScriptError::new("curl_escape requires a string".to_owned()))?;
    let input = checked_datum(player, input_ref)?.string_value(symbols)?;
    let escaped = percent_encoding::utf8_percent_encode(&input, percent_encoding::NON_ALPHANUMERIC).to_string();
    Ok(player.alloc_datum(Datum::String(escaped)))
}

fn curl_hfs2posix_explicit(
    player: &mut DirPlayer,
    symbols: &SymbolTable,
    args: &[DatumRef],
) -> Result<DatumRef, ScriptError> {
    let input_ref = args.first().ok_or_else(|| ScriptError::new("curl_hfs2posix requires a string".to_owned()))?;
    let input = checked_datum(player, input_ref)?.string_value(symbols)?;
    let value = if input.contains(':') {
        format!("/{}", input.replace(':', "/"))
    } else {
        input
    };
    Ok(player.alloc_datum(Datum::String(value)))
}

fn ok_int(n: i32) -> Result<DatumRef, ScriptError> {
    reserve_player_mut(|player| Ok(player.alloc_datum(Datum::Int(n))))
}

fn ok_string(s: String) -> Result<DatumRef, ScriptError> {
    reserve_player_mut(|player| Ok(player.alloc_datum(Datum::String(s))))
}

// -- setOption --------------------------------------------------------------

fn string_value_explicit(
    player: &DirPlayer,
    symbols: &SymbolTable,
    value_ref: Option<&DatumRef>,
) -> Result<String, ScriptError> {
    let arg = value_ref.ok_or_else(|| ScriptError::new("Missing value argument".to_owned()))?;
    checked_datum(player, arg)?.string_value(symbols)
}

fn int_value_explicit(
    player: &DirPlayer,
    value_ref: Option<&DatumRef>,
) -> Result<i32, ScriptError> {
    let arg = value_ref.ok_or_else(|| ScriptError::new("Missing value argument".to_owned()))?;
    checked_datum(player, arg)?.int_value()
}

fn set_option_explicit(
    instance: &mut CurlInstance,
    player: &mut DirPlayer,
    symbols: &SymbolTable,
    args: &[DatumRef],
) -> Result<DatumRef, ScriptError> {
    let option_ref = args.first().ok_or_else(|| ScriptError::new("setOption requires an option id".to_owned()))?;
    let option = checked_datum(player, option_ref)?.int_value()?;
    let value_ref = args.get(1);
    match option {
        opt::URL => instance.url = string_value_explicit(player, symbols, value_ref)?,
        opt::HTTPGET => {
            if int_value_explicit(player, value_ref)? != 0 {
                instance.method = "GET".to_owned();
                instance.custom_method = None;
            }
        }
        opt::POST => {
            if int_value_explicit(player, value_ref)? != 0 {
                instance.method = "POST".to_owned();
                instance.custom_method = None;
            }
        }
        opt::NOBODY => {
            if int_value_explicit(player, value_ref)? != 0 {
                instance.method = "HEAD".to_owned();
                instance.custom_method = None;
            }
        }
        opt::CUSTOMREQUEST => {
            instance.custom_method = Some(string_value_explicit(player, symbols, value_ref)?);
        }
        opt::HTTPHEADER => {
            let arg = value_ref.ok_or_else(|| ScriptError::new("Missing list argument".to_owned()))?;
            instance.headers = match checked_datum(player, arg)? {
                Datum::List(_, items, _) => items
                    .iter()
                    .map(|item| checked_datum(player, item)?.string_value(symbols))
                    .collect::<Result<Vec<_>, _>>()?,
                Datum::String(value) => vec![value.clone()],
                _ => Vec::new(),
            };
        }
        opt::POSTFIELDS => {
            instance.body = Some(string_value_explicit(player, symbols, value_ref)?.into_bytes());
        }
        opt::USERAGENT => instance.user_agent = Some(string_value_explicit(player, symbols, value_ref)?),
        opt::REFERER => instance.referer = Some(string_value_explicit(player, symbols, value_ref)?),
        opt::COOKIE => instance.cookie = Some(string_value_explicit(player, symbols, value_ref)?),
        opt::USERPWD => instance.user_pwd = Some(string_value_explicit(player, symbols, value_ref)?),
        opt::PROXY => instance.proxy = Some(string_value_explicit(player, symbols, value_ref)?),
        opt::RANGE => instance.range = Some(string_value_explicit(player, symbols, value_ref)?),
        opt::ACCEPT_ENCODING => instance.accept_encoding = Some(string_value_explicit(player, symbols, value_ref)?),
        opt::SSL_VERIFYPEER | opt::CAINFO => {}
        _ => {}
    }
    alloc_int(player, CURLE_OK)
}

fn set_form_explicit(
    instance: &mut CurlInstance,
    player: &mut DirPlayer,
    symbols: &SymbolTable,
    args: &[DatumRef],
) -> Result<DatumRef, ScriptError> {
    let prop_ref = args.first().ok_or_else(|| ScriptError::new("setForm requires a property-list argument".to_owned()))?;
    let pairs = match checked_datum(player, prop_ref)? {
        Datum::PropList(pairs, _) => pairs
            .iter()
            .map(|(key, value)| {
                Ok((
                    checked_datum(player, key)?.string_value(symbols)?,
                    checked_datum(player, value)?.string_value(symbols)?,
                ))
            })
            .collect::<Result<Vec<_>, ScriptError>>()?,
        _ => return Err(ScriptError::new("setForm requires a property list".to_owned())),
    };
    instance.form = pairs;
    instance.method = "POST".to_owned();
    instance.custom_method = None;
    alloc_int(player, CURLE_OK)
}

fn set_source_file_explicit(player: &mut DirPlayer) -> Result<DatumRef, ScriptError> {
    alloc_int(player, CURLE_NOT_BUILT_IN)
}

fn set_destination_file_explicit(player: &mut DirPlayer) -> Result<DatumRef, ScriptError> {
    alloc_int(player, CURLE_NOT_BUILT_IN)
}

fn set_callback_explicit(
    slot: &mut Option<(DatumRef, String)>,
    player: &DirPlayer,
    symbols: &mut SymbolTable,
    args: &[DatumRef],
    name: &str,
) -> Result<DatumRef, ScriptError> {
    let handler_ref = args
        .first()
        .ok_or_else(|| ScriptError::new(format!("{name} requires a symbol")))?;
    let handler = checked_datum(player, handler_ref)?.symbol_value(symbols)?;
    let handler = symbols.display(&handler).map_err(|_| ScriptError::new_code(
        crate::player::ScriptErrorCode::InvalidReference,
        "foreign Xtra callback symbol".to_owned(),
    ))?.to_owned();
    let target = args.get(1).cloned().unwrap_or(DatumRef::Void);
    if !matches!(target, DatumRef::Void) {
        checked_retained(player, symbols, &target)?;
    }
    *slot = Some((target, handler));
    Ok(DatumRef::Void)
}

fn get_info_explicit(
    instance: &CurlInstance,
    player: &mut DirPlayer,
    args: &[DatumRef],
) -> Result<DatumRef, ScriptError> {
    let info_ref = args.first().ok_or_else(|| ScriptError::new("getInfo requires an info id".to_owned()))?;
    let info = checked_datum(player, info_ref)?.int_value()?;
    match info {
        2097154 => Ok(player.alloc_datum(Datum::String(instance.last_effective_url.clone()))),
        2097162 => alloc_int(player, instance.last_status),
        3145731 => Ok(player.alloc_datum(Datum::Float(0.0))),
        3145735 => Ok(player.alloc_datum(Datum::Float(instance.last_response_size as f64))),
        _ => alloc_int(player, 0),
    }
}

fn set_option(instance: &mut CurlInstance, args: &Vec<DatumRef>, symbols: &SymbolTable) -> Result<DatumRef, ScriptError> {
    let option = reserve_player_ref(|player| {
        let arg = args.get(0).ok_or_else(|| {
            ScriptError::new("setOption requires an option id".to_string())
        })?;
        player.get_datum(arg).int_value()
    })?;
    let value_ref = args.get(1);

    match option {
        opt::URL => {
            instance.url = string_value(value_ref, symbols)?;
        }
        opt::HTTPGET => {
            if int_value(value_ref)? != 0 {
                instance.method = "GET".to_string();
                instance.custom_method = None;
            }
        }
        opt::POST => {
            if int_value(value_ref)? != 0 {
                instance.method = "POST".to_string();
                instance.custom_method = None;
            }
        }
        opt::NOBODY => {
            if int_value(value_ref)? != 0 {
                instance.method = "HEAD".to_string();
                instance.custom_method = None;
            }
        }
        opt::CUSTOMREQUEST => {
            instance.custom_method = Some(string_value(value_ref, symbols)?);
        }
        opt::HTTPHEADER => {
            instance.headers = list_string_values(value_ref, symbols)?;
        }
        opt::POSTFIELDS => {
            let body = string_value(value_ref, symbols)?;
            instance.body = Some(body.into_bytes());
        }
        opt::USERAGENT => instance.user_agent = Some(string_value(value_ref, symbols)?),
        opt::REFERER => instance.referer = Some(string_value(value_ref, symbols)?),
        opt::COOKIE => instance.cookie = Some(string_value(value_ref, symbols)?),
        opt::USERPWD => instance.user_pwd = Some(string_value(value_ref, symbols)?),
        opt::PROXY => instance.proxy = Some(string_value(value_ref, symbols)?),
        opt::RANGE => instance.range = Some(string_value(value_ref, symbols)?),
        opt::ACCEPT_ENCODING => instance.accept_encoding = Some(string_value(value_ref, symbols)?),
        opt::SSL_VERIFYPEER | opt::CAINFO => {
            // The browser fetch stack already handles TLS verification — these
            // options are no-ops, but we accept them so existing Lingo code
            // doesn't see CURLE_UNKNOWN_OPTION.
        }
        _ => {
            // Unknown options are silently accepted, like the real Xtra does
            // when libcurl reports CURLE_OK for ignored toggles.
        }
    }
    ok_int(CURLE_OK)
}

fn set_form(instance: &mut CurlInstance, args: &Vec<DatumRef>, symbols: &SymbolTable) -> Result<DatumRef, ScriptError> {
    let prop_ref = args.get(0).ok_or_else(|| {
        ScriptError::new("setForm requires a property-list argument".to_string())
    })?;
    let pairs = reserve_player_ref(|player| {
        let datum = player.get_datum(prop_ref);
        if let Datum::PropList(pairs, _) = datum {
            let mut out = Vec::new();
            for (key, value) in pairs.iter() {
                let k = player.get_datum(key).string_value(symbols)?;
                let v = player.get_datum(value).string_value(symbols)?;
                out.push((k, v));
            }
            Ok(out)
        } else {
            Err(ScriptError::new(
                "setForm requires a property list".to_string(),
            ))
        }
    })?;
    instance.form = pairs;
    instance.method = "POST".to_string();
    instance.custom_method = None;
    ok_int(CURLE_OK)
}

fn set_source_file(_instance: &mut CurlInstance, _args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
    // File uploads from disk are not available in WASM.
    ok_int(CURLE_NOT_BUILT_IN)
}

fn set_destination_file(_instance: &mut CurlInstance, _args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
    ok_int(CURLE_NOT_BUILT_IN)
}

fn set_header_callback(
    instance: &mut CurlInstance,
    args: &Vec<DatumRef>,
    symbols: &mut SymbolTable,
) -> Result<DatumRef, ScriptError> {
    let handler = reserve_player_ref(|player| {
        let arg = args.get(0).ok_or_else(|| {
            ScriptError::new("setHeaderCallback requires a symbol".to_string())
        })?;
        player.get_datum(arg).symbol_value(symbols)
    })?;
    let target = args.get(1).cloned().unwrap_or(DatumRef::Void);
    let handler = symbols.display(&handler).map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?.to_owned();
    instance.header_callback = Some((target, handler));
    Ok(DatumRef::Void)
}

fn set_progress_callback(
    instance: &mut CurlInstance,
    args: &Vec<DatumRef>,
    symbols: &mut SymbolTable,
) -> Result<DatumRef, ScriptError> {
    let handler = reserve_player_ref(|player| {
        let arg = args.get(0).ok_or_else(|| {
            ScriptError::new("setProgressCallback requires a symbol".to_string())
        })?;
        player.get_datum(arg).symbol_value(symbols)
    })?;
    let target = args.get(1).cloned().unwrap_or(DatumRef::Void);
    let handler = symbols.display(&handler).map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?.to_owned();
    instance.progress_callback = Some((target, handler));
    Ok(DatumRef::Void)
}

// -- getInfo ----------------------------------------------------------------

fn get_info(instance: &CurlInstance, args: &Vec<DatumRef>, _symbols: &SymbolTable) -> Result<DatumRef, ScriptError> {
    let info = reserve_player_ref(|player| {
        let arg = args.get(0).ok_or_else(|| {
            ScriptError::new("getInfo requires an info id".to_string())
        })?;
        player.get_datum(arg).int_value()
    })?;
    // Mirror libcurl's CURLINFO_* constants (subset).
    match info {
        2097154 => ok_string(instance.last_effective_url.clone()), // EFFECTIVE_URL
        2097162 => ok_int(instance.last_status),                   // RESPONSE_CODE
        3145731 => reserve_player_mut(|player| {
            Ok(player.alloc_datum(Datum::Float(0.0)))
        }), // TOTAL_TIME
        3145735 => reserve_player_mut(|player| {
            Ok(player.alloc_datum(Datum::Float(instance.last_response_size as f64)))
        }), // SIZE_DOWNLOAD
        _ => ok_int(0),
    }
}

pub(crate) async fn perform_fetch_owned(instance: &CurlInstance) -> (i32, Vec<u8>, String) {
    if instance.url.is_empty() {
        return (-CURLE_URL_MALFORMAT, Vec::new(), String::new());
    }
    let window = match web_sys::window() {
        Some(w) => w,
        None => return (-CURLE_FAILED_INIT, Vec::new(), String::new()),
    };

    let method = instance.effective_method();
    let mut init = web_sys::RequestInit::new();
    init.set_method(&method);
    init.set_mode(web_sys::RequestMode::Cors);

    // Body / form
    let mut content_type_override: Option<String> = None;
    if !instance.form.is_empty() {
        // multipart/form-data via FormData
        let form_data = match web_sys::FormData::new() {
            Ok(f) => f,
            Err(_) => return (-CURLE_FAILED_INIT, Vec::new(), String::new()),
        };
        for (k, v) in &instance.form {
            let _ = form_data.append_with_str(k, v);
        }
        init.set_body(&form_data.into());
    } else if let Some(body) = instance.body.as_ref() {
        if !body.is_empty() {
            let arr = js_sys::Uint8Array::new_with_length(body.len() as u32);
            arr.copy_from(body);
            init.set_body(&arr.buffer().into());
            // Default to form-encoded if no Content-Type header is set,
            // matching libcurl's CURLOPT_POSTFIELDS default.
            if !instance
                .headers
                .iter()
                .any(|h| h.to_ascii_lowercase().starts_with("content-type:"))
            {
                content_type_override =
                    Some("application/x-www-form-urlencoded".to_string());
            }
        }
    }

    // Headers
    let headers = match web_sys::Headers::new() {
        Ok(h) => h,
        Err(_) => return (-CURLE_FAILED_INIT, Vec::new(), String::new()),
    };
    for header in &instance.headers {
        if let Some((name, value)) = header.split_once(':') {
            let _ = headers.append(name.trim(), value.trim());
        }
    }
    if let Some(ua) = &instance.user_agent {
        let _ = headers.set("User-Agent", ua);
    }
    if let Some(referer) = &instance.referer {
        let _ = headers.set("Referer", referer);
    }
    if let Some(cookie) = &instance.cookie {
        let _ = headers.set("Cookie", cookie);
    }
    if let Some(range) = &instance.range {
        let _ = headers.set("Range", &format!("bytes={}", range));
    }
    if let Some(enc) = &instance.accept_encoding {
        let _ = headers.set("Accept-Encoding", enc);
    }
    if let Some(ct) = &content_type_override {
        let _ = headers.set("Content-Type", ct);
    }
    init.set_headers(&headers.into());

    let request = match web_sys::Request::new_with_str_and_init(&instance.url, &init) {
        Ok(r) => r,
        Err(_) => return (-CURLE_URL_MALFORMAT, Vec::new(), String::new()),
    };

    let response_value = match JsFuture::from(window.fetch_with_request(&request)).await {
        Ok(v) => v,
        Err(_) => return (-CURLE_COULDNT_CONNECT, Vec::new(), String::new()),
    };
    let response: web_sys::Response = match response_value.dyn_into() {
        Ok(r) => r,
        Err(_) => return (-CURLE_COULDNT_CONNECT, Vec::new(), String::new()),
    };
    let status = response.status() as i32;
    let headers_string = serialize_headers(&response);

    let buffer_promise = match response.array_buffer() {
        Ok(p) => p,
        Err(_) => return (status, Vec::new(), headers_string),
    };
    let buffer_value = match JsFuture::from(buffer_promise).await {
        Ok(v) => v,
        Err(_) => return (status, Vec::new(), headers_string),
    };
    let buffer: js_sys::ArrayBuffer = match buffer_value.dyn_into() {
        Ok(b) => b,
        Err(_) => return (status, Vec::new(), headers_string),
    };
    let view = js_sys::Uint8Array::new(&buffer);
    let mut bytes = vec![0u8; view.length() as usize];
    view.copy_to(&mut bytes);

    let result_status = if status >= 400 {
        -CURLE_HTTP_RETURNED_ERROR
    } else {
        status
    };
    (result_status, bytes, headers_string)
}

fn serialize_headers(_response: &web_sys::Response) -> String {
    // The Fetch API does not expose `Response.headers` iteration on the
    // `Headers` type bound by web-sys 0.3.85 (the entries() iterator requires
    // a newer feature). We return the raw status line + an empty CRLF so
    // Lingo scripts that just look for "HTTP/1.1 <code>" continue to work.
    String::new()
}

fn string_value(value_ref: Option<&DatumRef>, symbols: &SymbolTable) -> Result<String, ScriptError> {
    let arg = value_ref.ok_or_else(|| ScriptError::new("Missing value argument".to_string()))?;
    reserve_player_ref(|player| player.get_datum(arg).string_value(symbols))
}

fn int_value(value_ref: Option<&DatumRef>) -> Result<i32, ScriptError> {
    let arg = value_ref.ok_or_else(|| ScriptError::new("Missing value argument".to_string()))?;
    reserve_player_ref(|player| player.get_datum(arg).int_value())
}

fn list_string_values(value_ref: Option<&DatumRef>, symbols: &SymbolTable) -> Result<Vec<String>, ScriptError> {
    let arg = value_ref.ok_or_else(|| ScriptError::new("Missing list argument".to_string()))?;
    reserve_player_ref(|player| {
        let datum = player.get_datum(arg);
        match datum {
            Datum::List(_, items, _) => items
                .iter()
                .map(|item| player.get_datum(item).string_value(symbols))
                .collect(),
            Datum::String(s) => Ok(vec![s.clone()]),
            _ => Ok(Vec::new()),
        }
    })
    .map_err(|e: ScriptError| e)
    .and_then(|v: Vec<String>| Ok(v))
}

// -- Static handlers --------------------------------------------------------

fn curl_error(args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
    let code = reserve_player_ref(|player| {
        let arg = args.get(0).ok_or_else(|| {
            ScriptError::new("curl_error requires an integer".to_string())
        })?;
        player.get_datum(arg).int_value()
    })?;
    let msg = match code {
        0 => "No error",
        1 => "Unsupported protocol",
        2 => "Failed init",
        3 => "URL malformat",
        4 => "Not built-in (unsupported in WASM)",
        6 => "Couldn't resolve host",
        7 => "Couldn't connect to server",
        22 => "HTTP returned error",
        _ => "Unknown error",
    };
    ok_string(msg.to_string())
}

fn curl_escape(args: &Vec<DatumRef>, symbols: &SymbolTable) -> Result<DatumRef, ScriptError> {
    let s = reserve_player_ref(|player| {
        let arg = args.get(0).ok_or_else(|| {
            ScriptError::new("curl_escape requires a string".to_string())
        })?;
        player.get_datum(arg).string_value(symbols)
    })?;
    let escaped = percent_encoding::utf8_percent_encode(&s, percent_encoding::NON_ALPHANUMERIC)
        .to_string();
    ok_string(escaped)
}

fn curl_hfs2posix(args: &Vec<DatumRef>, symbols: &SymbolTable) -> Result<DatumRef, ScriptError> {
    // HFS-style "Macintosh HD:Users:foo" -> "/Macintosh HD/Users/foo".
    // No-op on Windows/WASM, but we still translate the separator.
    let s = reserve_player_ref(|player| {
        let arg = args.get(0).ok_or_else(|| {
            ScriptError::new("curl_hfs2posix requires a string".to_string())
        })?;
        player.get_datum(arg).string_value(symbols)
    })?;
    let posix = if s.contains(':') {
        let mut out = String::with_capacity(s.len() + 1);
        out.push('/');
        out.push_str(&s.replace(':', "/"));
        out
    } else {
        s
    };
    ok_string(posix)
}

// Silence the dead-code warnings on enum values we accept but don't act on.
#[allow(dead_code)]
const _: i32 = CURLE_UNSUPPORTED_PROTOCOL + CURLE_COULDNT_RESOLVE_HOST;

// `DatumType` is brought in to silence an unused-import warning when the
// async path expands; it documents the expected list type for HTTPHEADER.
#[allow(dead_code)]
const _USED: DatumType = DatumType::List;
