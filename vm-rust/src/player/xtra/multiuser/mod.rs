pub mod blowfish;
pub mod writer;
pub mod reader;
pub mod types;

use std::collections::VecDeque;
use async_std::{channel::Sender, task::spawn_local};
use binary_reader::BinaryReader;
use fxhash::FxHashMap;
use futures::FutureExt;
use itertools::Itertools;
use num::FromPrimitive;
use num_derive::FromPrimitive;
use wasm_bindgen::{closure::Closure, JsCast};
use web_sys::{CloseEvent, Event, MessageEvent, WebSocket};

// Use console::warn_1 directly for debugging since log level is set to Error
macro_rules! multiuser_log {
    ($($arg:tt)*) => {
        web_sys::console::warn_1(&format!($($arg)*).into())
    };
}

use crate::{
    director::{lingo::datum::{Datum, DatumType}, static_datum::StaticDatum},
    player::{
        DatumRef, DirPlayer, PlayerVMExecutionItem, ScriptError, events::player_dispatch_callback_event, reserve_player_mut, reserve_player_ref, symbols::{builtin::BuiltInSymbol, symbol::Symbol, symbol_table::SymbolTable}, xtra::manager::{MultiuserConnectRequest, MultiuserSendRequest, XtraPendingOrValue}, xtra::multiuser::blowfish::{DEFAULT_CIPHER_KEY, MUSBlowfish}
    },
};

#[derive(Debug, Clone, FromPrimitive)]
pub enum MultiuserConnectionMode {
    Binary = 0,
    Text = 1,
}

pub struct MultiuserMessage {
    pub error_code: i32,
    pub recipients: Vec<String>,
    pub sender_id: String,
    pub subject: String,
    pub content: StaticDatum,
    pub time_stamp: u32,
}

#[derive(Clone)]
pub(crate) enum MultiuserSocketEvent {
    Opened(MultiuserConnectRequest),
    Message(Vec<u8>),
    Error(String),
    Closed { code: u16, reason: String, was_clean: bool },
}

fn socket_event_command(
    owner: &crate::player::ownership::OwnerToken,
    instance_id: u32,
    generation: u64,
    event: MultiuserSocketEvent,
) -> PlayerVMExecutionItem {
    PlayerVMExecutionItem {
        command: crate::player::commands::PlayerVMCommand::MultiuserSocketEvent {
            owner: owner.clone(),
            instance_id,
            generation,
            event,
        },
        completer: None,
    }
}

/// Own the browser socket and its event closures together. Keeping the
/// closures in the instance (rather than forgetting them) makes reset and
/// generation retirement detach the listeners and close the socket when the
/// owner-bound instance is dropped.
#[cfg(target_arch = "wasm32")]
pub(crate) struct MultiuserSocketResource {
    socket: WebSocket,
    cancel_tx: Sender<()>,
    onmessage: Closure<dyn FnMut(MessageEvent)>,
    onerror: Closure<dyn FnMut(Event)>,
    onclose: Closure<dyn FnMut(CloseEvent)>,
    onopen: Closure<dyn FnMut(Event)>,
}

#[cfg(target_arch = "wasm32")]
impl Drop for MultiuserSocketResource {
    fn drop(&mut self) {
        let _ = self.cancel_tx.try_send(());
        self.socket.set_onmessage(None);
        self.socket.set_onerror(None);
        self.socket.set_onclose(None);
        self.socket.set_onopen(None);
        let _ = self.socket.close();
    }
}

pub struct MultiuserXtraInstance {
    pub generation: u64,
    pub net_message_handler: Option<(DatumRef, Symbol)>,
    pub message_queue: Vec<MultiuserMessage>,
    pub socket_tx: Option<Sender<Vec<u8>>>,
    pub connection_mode: MultiuserConnectionMode,
    pub sender_id: String,
    pub recv_buffer: Vec<u8>,
    #[cfg(target_arch = "wasm32")]
    socket_resource: Option<MultiuserSocketResource>,
    #[cfg(test)]
    pub(crate) teardown_probe: Option<std::rc::Rc<dyn Fn()>>,
}

struct ReleasedMultiuserInstance {
    instance_id: u32,
    generation: u64,
    #[cfg(target_arch = "wasm32")]
    socket_resource: Option<MultiuserSocketResource>,
    #[cfg(test)]
    teardown_probe: Option<std::rc::Rc<dyn Fn()>>,
}

pub(crate) struct MultiuserConnectPreparation {
    pub(crate) owner: crate::player::ownership::OwnerToken,
    pub(crate) instance_id: u32,
    pub(crate) generation: u64,
    pub(crate) request: MultiuserConnectRequest,
    pub(crate) queue: Sender<PlayerVMExecutionItem>,
}

#[cfg(target_arch = "wasm32")]
pub(crate) struct ConnectedMultiuserSocket {
    pub(crate) owner: crate::player::ownership::OwnerToken,
    pub(crate) instance_id: u32,
    pub(crate) generation: u64,
    pub(crate) username: String,
    pub(crate) connection_mode: MultiuserConnectionMode,
    pub(crate) socket_tx: Sender<Vec<u8>>,
    pub(crate) socket_resource: MultiuserSocketResource,
}

impl MultiuserXtraInstance {
    pub fn dispatch_message_handler(&self) {
        if let Some((handler_obj_ref, handler_symbol)) = &self.net_message_handler {
            let handler_symbol = handler_symbol.clone();
            let handler_obj_ref = handler_obj_ref.clone();
            player_dispatch_callback_event(handler_obj_ref, handler_symbol, &vec![]);
        }
    }

    pub fn dispatch_message(&mut self, message: MultiuserMessage) {
        self.message_queue.push(message);
        self.dispatch_message_handler();
    }

    fn push_message_without_callback(&mut self, message: MultiuserMessage) {
        self.message_queue.push(message);
    }

    pub fn next_message(&mut self) -> Option<MultiuserMessage> {
        if self.message_queue.is_empty() {
            return None;
        }
        Some(self.message_queue.remove(0))
    }

    pub fn receive_binary_data(&mut self, data: &[u8]) {
        use crate::player::xtra::multiuser::reader::{MUS_HEADER, MUS_FRAME_HEADER_SIZE, MusReader};

        self.recv_buffer.extend_from_slice(data);
        loop {
            if self.recv_buffer.len() < MUS_FRAME_HEADER_SIZE {
                break;
            }
            let header = u16::from_be_bytes([self.recv_buffer[0], self.recv_buffer[1]]);
            if header != MUS_HEADER {
                multiuser_log!("Multiuser: Invalid message header 0x{:04x}, discarding recv buffer", header);
                self.recv_buffer.clear();
                break;
            }
            let payload_size = u32::from_be_bytes([
                self.recv_buffer[2], self.recv_buffer[3],
                self.recv_buffer[4], self.recv_buffer[5],
            ]) as usize;
            let total_size = MUS_FRAME_HEADER_SIZE + payload_size;
            if self.recv_buffer.len() < total_size {
                break;
            }
            let msg_bytes: Vec<u8> = self.recv_buffer.drain(..total_size).collect();
            let payload = &msg_bytes[MUS_FRAME_HEADER_SIZE..];
            match BinaryReader::read_mus_message_payload(payload, None) {
                Ok(msg) => self.dispatch_message(msg),
                Err(e) => multiuser_log!("Multiuser: Failed to read binary message: {:?}", e),
            }
        }
    }

    fn receive_binary_data_without_callback(&mut self, data: &[u8]) {
        use crate::player::xtra::multiuser::reader::{MUS_HEADER, MUS_FRAME_HEADER_SIZE, MusReader};
        self.recv_buffer.extend_from_slice(data);
        loop {
            if self.recv_buffer.len() < MUS_FRAME_HEADER_SIZE { break; }
            let header = u16::from_be_bytes([self.recv_buffer[0], self.recv_buffer[1]]);
            if header != MUS_HEADER { self.recv_buffer.clear(); break; }
            let payload_size = u32::from_be_bytes([
                self.recv_buffer[2], self.recv_buffer[3], self.recv_buffer[4], self.recv_buffer[5],
            ]) as usize;
            let total_size = MUS_FRAME_HEADER_SIZE + payload_size;
            if self.recv_buffer.len() < total_size { break; }
            let msg_bytes: Vec<u8> = self.recv_buffer.drain(..total_size).collect();
            match BinaryReader::read_mus_message_payload(&msg_bytes[MUS_FRAME_HEADER_SIZE..], None) {
                Ok(message) => self.push_message_without_callback(message),
                Err(error) => multiuser_log!("Multiuser: Failed to read binary message: {:?}", error),
            }
        }
    }
}

pub struct MultiuserXtraManager {
    pub instances: FxHashMap<u32, MultiuserXtraInstance>,
    pub instance_counter: u32,
    pub owner: crate::player::ownership::OwnerToken,
    pub generation_counter: u64,
    released_instances: Vec<ReleasedMultiuserInstance>,
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

fn checked_retained(
    player: &DirPlayer,
    symbols: &SymbolTable,
    datum: &DatumRef,
) -> Result<(), ScriptError> {
    let value = checked_datum(player, datum)?;
    crate::player::compare::validate_direct_symbol_fields(value, symbols)?;
    if let Datum::ScriptInstanceRef(instance) = value {
        use crate::player::allocator::ScriptInstanceAllocatorTrait;
        player.allocator.get_script_instance_opt(instance).ok_or_else(|| {
            ScriptError::new_code(
                crate::player::ScriptErrorCode::InvalidReference,
                "foreign or stale Multiuser callback target".to_owned(),
            )
        })?;
    }
    Ok(())
}

fn required_arg<'a>(player: &'a DirPlayer, args: &'a [DatumRef], index: usize) -> Result<&'a Datum, ScriptError> {
    let arg = args.get(index).ok_or_else(|| ScriptError::new(format!("missing Multiuser argument {}", index + 1)))?;
    checked_datum(player, arg)
}

fn string_arg(player: &DirPlayer, symbols: &SymbolTable, args: &[DatumRef], index: usize) -> Result<String, ScriptError> {
    let value = required_arg(player, args, index)?;
    crate::player::compare::validate_direct_symbol_fields(value, symbols)?;
    value.string_value(symbols)
}

fn int_arg(player: &DirPlayer, symbols: &SymbolTable, args: &[DatumRef], index: usize) -> Result<i32, ScriptError> {
    let value = required_arg(player, args, index)?;
    crate::player::compare::validate_direct_symbol_fields(value, symbols)?;
    value.int_value()
}

fn optional_int_fallback(player: &DirPlayer, symbols: &SymbolTable, args: &[DatumRef], index: usize) -> Result<i32, ScriptError> {
    args.get(index)
        .map(|arg| {
            let value = checked_datum(player, arg)?;
            crate::player::compare::validate_direct_symbol_fields(value, symbols)?;
            Ok(value.int_value().unwrap_or(0))
        })
        .transpose()
        .map(|value| value.unwrap_or(0))
}

fn optional_string_fallback(player: &DirPlayer, symbols: &SymbolTable, args: &[DatumRef], index: usize) -> Result<String, ScriptError> {
    args.get(index)
        .map(|arg| {
            let value = checked_datum(player, arg)?;
            crate::player::compare::validate_direct_symbol_fields(value, symbols)?;
            Ok(value.string_value(symbols).unwrap_or_default())
        })
        .transpose()
        .map(|value| value.unwrap_or_default())
}

fn first_property_string(
    player: &DirPlayer,
    symbols: &mut SymbolTable,
    properties: &VecDeque<(DatumRef, DatumRef)>,
    wanted: BuiltInSymbol,
) -> Result<String, ScriptError> {
    for (key_ref, value_ref) in properties {
        let key = match checked_datum(player, key_ref)? {
            Datum::Symbol(_) | Datum::String(_) => {
                let key = checked_datum(player, key_ref)?.symbol_value(symbols)?;
                key.into_builtin_or_error(symbols).ok()
            }
            _ => None,
        };
        if key == Some(wanted) {
            return Ok(checked_datum(player, value_ref)?.string_value(symbols).unwrap_or_default());
        }
    }
    Ok(String::new())
}

fn owned_static_datum(
    player: &DirPlayer,
    symbols: &SymbolTable,
    datum: &Datum,
) -> Result<crate::director::static_datum::StaticDatum, ScriptError> {
    use crate::director::static_datum::StaticDatum;
    Ok(match datum {
        Datum::Int(value) => StaticDatum::Int(*value),
        Datum::Float(value) => StaticDatum::Float(*value),
        Datum::String(value) => StaticDatum::String(value.clone()),
        Datum::Symbol(value) => StaticDatum::Symbol(symbols.display(value).map_err(|_| ScriptError::new_code(crate::player::ScriptErrorCode::InvalidReference, "foreign Multiuser symbol".to_owned()))?.to_owned()),
        Datum::List(_, values, _) => StaticDatum::List(values.iter().map(|value| owned_static_datum(player, symbols, checked_datum(player, value)?)).collect::<Result<_, _>>()?),
        Datum::PropList(values, _) => StaticDatum::PropList(values.iter().map(|(key, value)| Ok((
            owned_static_datum(player, symbols, checked_datum(player, key)?)?,
            owned_static_datum(player, symbols, checked_datum(player, value)?)?,
        ))).collect::<Result<_, ScriptError>>()?),
        Datum::Point(values, _) => StaticDatum::IntPoint(values[0] as i32, values[1] as i32),
        Datum::Rect(values, _) => StaticDatum::IntRect(values[0] as i32, values[1] as i32, values[2] as i32, values[3] as i32),
        Datum::Media(_) => crate::director::static_datum::static_datum_from_datum(player, symbols, datum)?,
        _ => StaticDatum::Void,
    })
}

fn prepare_connect_request(
    player: &DirPlayer,
    symbols: &mut SymbolTable,
    args: &[DatumRef],
) -> Result<MultiuserConnectRequest, ScriptError> {
    if matches!(required_arg(player, args, 2)?, Datum::PropList(..)) {
        let host = string_arg(player, symbols, args, 0)?;
        let port = int_arg(player, symbols, args, 1)?;
        let plist = required_arg(player, args, 2)?;
        let mut username = String::new();
        let mut password = String::new();
        let mut movie_id = String::new();
        if let Datum::PropList(pairs, _) = plist {
            username = first_property_string(player, symbols, pairs, BuiltInSymbol::UserID)?;
            password = first_property_string(player, symbols, pairs, BuiltInSymbol::Password)?;
            movie_id = first_property_string(player, symbols, pairs, BuiltInSymbol::Movieid)?;
        }
        Ok(MultiuserConnectRequest { username, password, host, port, movie_id, mode: optional_int_fallback(player, symbols, args, 3)?, encryption_key: optional_string_fallback(player, symbols, args, 4)?, websocket_path: player.external_params.get("multiuser_websocket_path").cloned(), websocket_ssl: player.external_params.get("multiuser_websocket_ssl").filter(|value| !value.is_empty()).map(|value| value == "true" || value == "1") })
    } else {
        Ok(MultiuserConnectRequest {
            username: string_arg(player, symbols, args, 0).unwrap_or_default(),
            password: string_arg(player, symbols, args, 1).unwrap_or_default(),
            host: string_arg(player, symbols, args, 2)?,
            port: int_arg(player, symbols, args, 3)?,
            movie_id: string_arg(player, symbols, args, 4).unwrap_or_default(),
            mode: optional_int_fallback(player, symbols, args, 5)?,
            encryption_key: optional_string_fallback(player, symbols, args, 6)?,
            websocket_path: player.external_params.get("multiuser_websocket_path").cloned(),
            websocket_ssl: player.external_params.get("multiuser_websocket_ssl").filter(|value| !value.is_empty()).map(|value| value == "true" || value == "1"),
        })
    }
}

fn prepare_send_request(
    player: &DirPlayer,
    symbols: &SymbolTable,
    instance: &MultiuserXtraInstance,
    args: &[DatumRef],
) -> Result<MultiuserSendRequest, ScriptError> {
    if matches!(instance.connection_mode, MultiuserConnectionMode::Text) {
        return Ok(MultiuserSendRequest::Text { message: string_arg(player, symbols, args, 2)? });
    }
    let subject = string_arg(player, symbols, args, 1)?;
    let content = owned_static_datum(player, symbols, required_arg(player, args, 2)?)?;
    let recipients = match required_arg(player, args, 0)? {
        Datum::List(_, values, _) => values.iter().map(|value| {
            let value = checked_datum(player, value)?;
            crate::player::compare::validate_direct_symbol_fields(value, symbols)?;
            Ok(value.string_value(symbols).unwrap_or_default())
        }).collect::<Result<_, ScriptError>>()?,
        Datum::String(value) => vec![value.clone()],
        Datum::Int(0) => Vec::new(),
        _ => return Err(ScriptError::new("Invalid recipients argument, expected list or string".to_owned())),
    };
    Ok(MultiuserSendRequest::Binary {
        recipients,
        subject,
        content,
        sender_id: instance.sender_id.clone(),
    })
}

impl MultiuserXtraManager {
    /// Send an already-owned payload through the live socket.  The caller has
    /// released the VM borrow before invoking this method and revalidates the
    /// owner/generation at the same boundary; no callback or global manager is
    /// consulted here.
    pub(crate) fn send_owned(
        &mut self,
        instance_id: u32,
        generation: u64,
        request: MultiuserSendRequest,
    ) -> Result<(), ScriptError> {
        let instance = self.instances.get_mut(&instance_id).ok_or_else(|| {
            ScriptError::new(format!("Multiuser instance #{} not found", instance_id))
        })?;
        if instance.generation != generation {
            return Err(ScriptError::new_code(
                crate::player::ScriptErrorCode::InvalidReference,
                "stale Multiuser instance generation".to_owned(),
            ));
        }
        let Some(tx) = instance.socket_tx.as_ref() else {
            return Err(ScriptError::new("Socket not connected".to_owned()));
        };
        let bytes = match request {
            MultiuserSendRequest::Text { message } => message.into_bytes(),
            MultiuserSendRequest::Binary { recipients, subject, content, sender_id } => {
                let message = MultiuserMessage {
                    error_code: 0,
                    recipients,
                    sender_id,
                    subject,
                    content,
                    time_stamp: 0,
                };
                message.to_bytes(None)
            }
        };
        tx.try_send(bytes).map_err(|_| ScriptError::new("Multiuser socket send queue is closed".to_owned()))
    }

    pub(crate) fn handle_socket_event(
        &mut self,
        instance_id: u32,
        generation: u64,
        event: MultiuserSocketEvent,
        queue: &Sender<PlayerVMExecutionItem>,
        owner: &crate::player::ownership::OwnerToken,
    ) -> Result<(), ScriptError> {
        // Browser callbacks can arrive after close/reset, and an old socket
        // must never mutate a replacement instance.  Treat both cases as a
        // successful discard before touching the instance or scheduling a
        // callback; the command owner boundary has already been checked.
        let Some(instance) = self.instances.get_mut(&instance_id) else {
            return Ok(());
        };
        if instance.generation != generation {
            return Ok(());
        }
        match event {
            MultiuserSocketEvent::Opened(request) => {
                instance.push_message_without_callback(MultiuserMessage {
                    error_code: 0,
                    recipients: vec!["*".to_owned()],
                    sender_id: "System".to_owned(),
                    subject: "ConnectToNetServer".to_owned(),
                    content: StaticDatum::Void,
                    time_stamp: 0,
                });
                if matches!(instance.connection_mode, MultiuserConnectionMode::Binary) {
                    let message = MultiuserMessage {
                        error_code: 0,
                        recipients: vec!["System".to_owned()],
                        sender_id: request.username.clone(),
                        subject: "Logon".to_owned(),
                        content: StaticDatum::List(vec![
                            StaticDatum::String(request.movie_id),
                            StaticDatum::String(request.username),
                            StaticDatum::String(request.password),
                        ]),
                        time_stamp: 0,
                    };
                    let key = if request.encryption_key.is_empty() {
                        DEFAULT_CIPHER_KEY.to_vec()
                    } else if request.encryption_key.len() < 20 {
                        let mut key = DEFAULT_CIPHER_KEY.to_vec();
                        key.extend_from_slice(request.encryption_key.as_bytes());
                        key
                    } else {
                        request.encryption_key.as_bytes().to_vec()
                    };
                    let mut cipher = MUSBlowfish::new(&key);
                    if let Some(tx) = instance.socket_tx.as_ref() {
                    tx.try_send(message.to_bytes(Some(&mut cipher))).map_err(|_| {
                            ScriptError::new("Multiuser socket send queue is closed".to_owned())
                        })?;
                    }
                }
            }
            MultiuserSocketEvent::Message(bytes) => match instance.connection_mode {
                MultiuserConnectionMode::Binary => instance.receive_binary_data_without_callback(&bytes),
                MultiuserConnectionMode::Text => instance.push_message_without_callback(MultiuserMessage {
                    error_code: 0,
                    recipients: Vec::new(),
                    sender_id: String::new(),
                    subject: String::new(),
                    content: StaticDatum::String(String::from_utf8_lossy(&bytes).into_owned()),
                    time_stamp: 0,
                }),
            },
            MultiuserSocketEvent::Error(message) => instance.push_message_without_callback(MultiuserMessage {
                error_code: -1,
                recipients: Vec::new(),
                sender_id: "System".to_owned(),
                subject: "ConnectToNetServer".to_owned(),
                content: StaticDatum::String(message),
                time_stamp: 0,
            }),
            MultiuserSocketEvent::Closed { code, reason, was_clean } => instance.push_message_without_callback(MultiuserMessage {
                error_code: if was_clean { 0 } else { -(code as i32) },
                recipients: Vec::new(),
                sender_id: "System".to_owned(),
                subject: "Close".to_owned(),
                content: StaticDatum::String(reason),
                time_stamp: 0,
            }),
        }
        if let Some((target, handler_name)) = instance.net_message_handler.clone() {
            queue.try_send(PlayerVMExecutionItem {
                command: crate::player::commands::PlayerVMCommand::TriggerXtraCallback {
                    owner: owner.clone(),
                    target,
                    handler_name,
                    args: Vec::new(),
                },
                completer: None,
            }).map_err(|_| ScriptError::new("Multiuser callback queue is closed".to_owned()))?;
        }
        Ok(())
    }

    pub(crate) fn prepare_connect_owned(
        &self,
        instance_id: u32,
        generation: u64,
        owner: crate::player::ownership::OwnerToken,
        request: MultiuserConnectRequest,
        queue: Sender<PlayerVMExecutionItem>,
    ) -> Result<MultiuserConnectPreparation, ScriptError> {
        if !self.owner.same_identity(&owner) || !owner.is_arena_live() {
            return Err(ScriptError::new_code(crate::player::ScriptErrorCode::InvalidReference, "stale Multiuser owner".to_owned()));
        }
        let instance = self.instances.get(&instance_id).ok_or_else(|| ScriptError::new(format!("Multiuser instance #{} not found", instance_id)))?;
        if instance.generation != generation {
            return Err(ScriptError::new_code(crate::player::ScriptErrorCode::InvalidReference, "stale Multiuser instance generation".to_owned()));
        }
        MultiuserConnectionMode::from_i32(request.mode)
            .ok_or_else(|| ScriptError::new(format!("Invalid connection mode: {}", request.mode)))?;
        Ok(MultiuserConnectPreparation { owner, instance_id, generation, request, queue })
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn install_connect_owned(
        &mut self,
        preparation: &MultiuserConnectPreparation,
        connected: &mut Option<ConnectedMultiuserSocket>,
    ) -> Result<Option<MultiuserSocketResource>, ScriptError> {
        if !self.owner.same_identity(&preparation.owner) || !preparation.owner.is_arena_live() {
            return Err(ScriptError::new_code(crate::player::ScriptErrorCode::InvalidReference, "stale Multiuser owner".to_owned()));
        }
        let instance = self.instances.get_mut(&preparation.instance_id).ok_or_else(|| ScriptError::new(format!("Multiuser instance #{} not found", preparation.instance_id)))?;
        if instance.generation != preparation.generation {
            return Err(ScriptError::new_code(crate::player::ScriptErrorCode::InvalidReference, "stale Multiuser instance generation".to_owned()));
        }
        let Some(connected_ref) = connected.as_ref() else {
            return Err(ScriptError::new("missing Multiuser connection result".to_owned()));
        };
        if !connected_ref.owner.same_identity(&preparation.owner)
            || connected_ref.instance_id != preparation.instance_id
            || connected_ref.generation != preparation.generation
        {
            return Err(ScriptError::new_code(crate::player::ScriptErrorCode::InvalidReference, "foreign Multiuser connection result".to_owned()));
        }
        let connected = connected.take().expect("validated Multiuser connection result");
        instance.sender_id = connected.username;
        instance.connection_mode = connected.connection_mode;
        instance.socket_tx = Some(connected.socket_tx);
        Ok(instance.socket_resource.replace(connected.socket_resource))
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn execute_connect_owned(
        preparation: &MultiuserConnectPreparation,
    ) -> Result<ConnectedMultiuserSocket, ScriptError> {
        let owner = preparation.owner.clone();
        let instance_id = preparation.instance_id;
        let generation = preparation.generation;
        let request = preparation.request.clone();
        let queue = preparation.queue.clone();
        let secure = request.websocket_ssl.unwrap_or_else(|| {
            web_sys::window().and_then(|window| window.location().protocol().ok()).is_some_and(|protocol| protocol == "https:")
        });
        let scheme = if secure { "wss" } else { "ws" };
        let default_url = format!("{}://{}:{}{}", scheme, request.host, request.port, request.websocket_path.clone().unwrap_or_default());
        let url = web_sys::window()
            .and_then(|window| js_sys::Reflect::get(&window, &"dirplayerResolveSocketUrl".into()).ok())
            .and_then(|resolver| resolver.dyn_ref::<js_sys::Function>().cloned())
            .and_then(|resolver| resolver.call2(&wasm_bindgen::JsValue::NULL, &request.host.as_str().into(), &wasm_bindgen::JsValue::from_f64(request.port as f64)).ok())
            .and_then(|value| value.as_string())
            .filter(|value| !value.is_empty())
            .unwrap_or(default_url);
        let socket = WebSocket::new(&url).map_err(|error| ScriptError::new(format!("failed to create Multiuser WebSocket: {:?}", error)))?;
        socket.set_binary_type(web_sys::BinaryType::Arraybuffer);
        let connection_mode = MultiuserConnectionMode::from_i32(request.mode).ok_or_else(|| ScriptError::new(format!("Invalid connection mode: {}", request.mode)))?;
        let queue_message = queue.clone();
        let event_owner = owner.clone();
        let onmessage = Closure::<dyn FnMut(_)>::new(move |message: MessageEvent| {
            let data = message.data();
            let bytes = if let Ok(buffer) = data.clone().dyn_into::<js_sys::ArrayBuffer>() {
                js_sys::Uint8Array::new(&buffer).to_vec()
            } else if let Some(text) = data.as_string() {
                text.into_bytes()
            } else {
                return;
            };
            let _ = queue_message.try_send(socket_event_command(&event_owner, instance_id, generation, MultiuserSocketEvent::Message(bytes)));
        });
        let queue_error = queue.clone();
        let event_owner = owner.clone();
        let onerror = Closure::<dyn FnMut(_)>::new(move |_error: Event| {
            let _ = queue_error.try_send(socket_event_command(&event_owner, instance_id, generation, MultiuserSocketEvent::Error("Multiuser WebSocket error".to_owned())));
        });
        let queue_close = queue.clone();
        let event_owner = owner.clone();
        let onclose = Closure::<dyn FnMut(_)>::new(move |close: CloseEvent| {
            let _ = queue_close.try_send(socket_event_command(&event_owner, instance_id, generation, MultiuserSocketEvent::Closed { code: close.code(), reason: close.reason(), was_clean: close.was_clean() }));
        });
        let queue_open = queue;
        let open_request = request.clone();
        let event_owner = owner.clone();
        let onopen = Closure::<dyn FnMut(_)>::new(move |_open: Event| {
            let _ = queue_open.try_send(socket_event_command(&event_owner, instance_id, generation, MultiuserSocketEvent::Opened(open_request.clone())));
        });
        socket.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        socket.set_onerror(Some(onerror.as_ref().unchecked_ref()));
        socket.set_onclose(Some(onclose.as_ref().unchecked_ref()));
        socket.set_onopen(Some(onopen.as_ref().unchecked_ref()));
        let socket_send = socket.clone();
        let (tx, rx) = async_std::channel::unbounded::<Vec<u8>>();
        let (cancel_tx, cancel_rx) = async_std::channel::bounded::<()>(1);
        // The pump only owns the browser socket and its cancellation channel;
        // it never touches VM state, so it must not capture the ambient active
        // player.  MultiuserSocketResource cancels this task when the owning
        // instance is retired or a rejected install is dropped.
        spawn_local(async move {
            loop {
                futures::select! {
                    message = rx.recv().fuse() => match message {
                        Ok(bytes) => {
                            if socket_send.ready_state() == 1 {
                                let _ = socket_send.send_with_u8_array(&bytes);
                            }
                        }
                        Err(_) => break,
                    },
                    _ = cancel_rx.recv().fuse() => break,
                }
            }
        });
        Ok(ConnectedMultiuserSocket {
            owner,
            instance_id,
            generation,
            username: request.username,
            connection_mode,
            socket_tx: tx,
            socket_resource: MultiuserSocketResource {
            socket,
            cancel_tx,
            onmessage,
            onerror,
            onclose,
            onopen,
            },
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn execute_connect_owned(
        _preparation: &MultiuserConnectPreparation,
    ) -> Result<(), ScriptError> {
        Err(ScriptError::new("Multiuser WebSocket executor unavailable on native target".to_owned()))
    }

    pub(crate) fn instance_generation(&self, instance_id: u32) -> Option<u64> {
        self.instances.get(&instance_id).map(|instance| instance.generation)
    }

    #[cfg(test)]
    pub(crate) fn message_state_for_test(&self, instance_id: u32) -> Option<Vec<(i32, String)>> {
        self.instances.get(&instance_id).map(|instance| {
            instance
                .message_queue
                .iter()
                .map(|message| {
                    let content = match &message.content {
                        StaticDatum::String(value) => value.clone(),
                        _ => String::new(),
                    };
                    (message.error_code, content)
                })
                .collect()
        })
    }

    pub(crate) fn create_instance_explicit(
        &mut self,
        player: &DirPlayer,
        args: &[DatumRef],
    ) -> Result<u32, ScriptError> {
        if !self.owner.same_identity(&player.owner) || !player.owner.is_arena_live() {
            return Err(ScriptError::new_code(
                crate::player::ScriptErrorCode::InvalidReference,
                "foreign or stale Multiuser manager owner".to_owned(),
            ));
        }
        let _ = args;
        self.instance_counter = self.instance_counter.wrapping_add(1);
        self.generation_counter = self.generation_counter.wrapping_add(1);
        self.instances.insert(self.instance_counter, MultiuserXtraInstance {
            generation: self.generation_counter,
            net_message_handler: None,
            message_queue: Vec::new(),
            socket_tx: None,
            connection_mode: MultiuserConnectionMode::Binary,
            sender_id: String::new(),
            recv_buffer: Vec::new(),
            #[cfg(target_arch = "wasm32")]
            socket_resource: None,
            #[cfg(test)]
            teardown_probe: None,
        });
        Ok(self.instance_counter)
    }

    pub(crate) fn call_instance_handler_or_pending_explicit(
        &mut self,
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        instance_id: u32,
        handler_name: &str,
        args: &[DatumRef],
    ) -> Result<crate::player::xtra::manager::XtraPendingOrValue, ScriptError> {
        if handler_name.eq_ignore_ascii_case("connectToNetServer")
            || handler_name.eq_ignore_ascii_case("sendNetMessage")
        {
            if !self.owner.same_identity(&player.owner) || !player.owner.is_arena_live() {
                return Err(ScriptError::new_code(
                    crate::player::ScriptErrorCode::InvalidReference,
                    "foreign or stale Multiuser manager owner".to_owned(),
                ));
            }
            let generation = self.instances.get(&instance_id).ok_or_else(|| {
                ScriptError::new(format!("Multiuser instance #{} not found", instance_id))
            })?.generation;
            let pending = if handler_name.eq_ignore_ascii_case("connectToNetServer") {
                let request = prepare_connect_request(player, symbols, args)?;
                crate::player::xtra::manager::XtraPendingIntent::MultiuserConnect {
                    owner: self.owner.clone(), instance_id, generation, request,
                }
            } else {
                let instance = self.instances.get(&instance_id).ok_or_else(|| {
                    ScriptError::new(format!("Multiuser instance #{} not found", instance_id))
                })?;
                // Preserve the legacy short-circuit: sendNetMessage checks the
                // live socket before converting any payload or ignored args.
                if instance.socket_tx.is_none() {
                    return Err(ScriptError::new("Socket not connected".to_owned()));
                }
                let request = prepare_send_request(player, symbols, instance, args)?;
                crate::player::xtra::manager::XtraPendingIntent::MultiuserSend {
                    owner: self.owner.clone(), instance_id, generation, request,
                }
            };
            return Ok(crate::player::xtra::manager::XtraPendingOrValue::Pending(pending));
        }
        self.call_instance_handler_explicit(player, symbols, instance_id, handler_name, args)
            .map(crate::player::xtra::manager::XtraPendingOrValue::Value)
    }

    pub(crate) fn call_instance_handler_explicit(
        &mut self,
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        instance_id: u32,
        handler_name: &str,
        args: &[DatumRef],
    ) -> Result<DatumRef, ScriptError> {
        if !self.owner.same_identity(&player.owner) || !player.owner.is_arena_live() {
            return Err(ScriptError::new_code(
                crate::player::ScriptErrorCode::InvalidReference,
                "foreign or stale Multiuser manager owner".to_owned(),
            ));
        }
        let instance = self.instances.get_mut(&instance_id).ok_or_else(|| {
            ScriptError::new(format!("Multiuser instance #{} not found", instance_id))
        })?;
        match handler_name.to_lowercase().as_str() {
            "setnetbufferlimits" | "checknetmessages" => Ok(DatumRef::Void),
            "setnetmessagehandler" => {
                let handler = args.first().ok_or_else(|| ScriptError::new("setNetMessageHandler requires a symbol".to_owned()))?;
                let handler = match checked_datum(player, handler)? {
                    Datum::Void => None,
                    _ => {
                        let symbol = checked_datum(player, handler)?.symbol_value(symbols)?;
                        Some((args.get(1).cloned().unwrap_or(DatumRef::Void), symbol))
                    }
                };
                if let Some((target, _)) = &handler {
                    if !matches!(target, DatumRef::Void) {
                        checked_retained(player, symbols, target)?;
                    }
                }
                instance.net_message_handler = handler;
                Ok(player.alloc_datum(Datum::Int(0)))
            }
            "getnetmessage" => {
                let Some(message) = instance.next_message() else { return Ok(DatumRef::Void); };
                let recipients = message.recipients.iter()
                    .map(|value| player.alloc_datum(Datum::String(value.clone())))
                    .collect::<VecDeque<_>>();
                let values = [
                    ("errorCode", player.alloc_datum(Datum::Int(message.error_code))),
                    ("recipients", player.alloc_datum(Datum::List(DatumType::List, recipients, false))),
                    ("senderID", player.alloc_datum(Datum::String(message.sender_id))),
                    ("subject", player.alloc_datum(Datum::String(message.subject))),
                    ("content", crate::director::static_datum::static_datum_to_runtime_with_symbols(&message.content, symbols, &mut player.allocator, &mut player.bitmap_manager)),
                    ("timeStamp", player.alloc_datum(Datum::Int(message.time_stamp as i32))),
                ];
                let refs = values.into_iter().map(|(name, value)| {
                    let key = player.alloc_datum(Datum::Symbol(symbols.intern(name)));
                    (key, value)
                }).collect();
                Ok(player.alloc_datum(Datum::PropList(refs, false)))
            }
            "getnumberwaitingnetmessages" => Ok(player.alloc_datum(Datum::Int(instance.message_queue.len() as i32))),
            "breakconnection" => {
                let retired_generation = instance.generation;
                instance.socket_tx = None;
                if !self.released_instances.iter().any(|released| {
                    released.instance_id == instance_id && released.generation == retired_generation
                }) {
                    self.released_instances.push(ReleasedMultiuserInstance {
                        instance_id,
                        generation: retired_generation,
                        #[cfg(target_arch = "wasm32")]
                        socket_resource: instance.socket_resource.take(),
                        #[cfg(test)]
                        teardown_probe: instance.teardown_probe.take(),
                    });
                }
                self.generation_counter = self.generation_counter.wrapping_add(1);
                instance.generation = self.generation_counter;
                Ok(DatumRef::Void)
            }
            "close" => {
                self.released_instances.push(ReleasedMultiuserInstance {
                    instance_id,
                    generation: instance.generation,
                    #[cfg(target_arch = "wasm32")]
                    socket_resource: instance.socket_resource.take(),
                    #[cfg(test)]
                    teardown_probe: instance.teardown_probe.take(),
                });
                self.instances.remove(&instance_id);
                Ok(DatumRef::Void)
            }
            "getnetaddresscookie" => Ok(player.alloc_datum(Datum::String(String::new()))),
            "getneterrorstring" => {
                let code = checked_datum(player, args.first().ok_or_else(|| ScriptError::new("getNetErrorString requires an error code".to_owned()))?)?.int_value()?;
                let text = match code { 0 => "No error", -1 => "Connection failed", -2 => "Connection refused", -3 => "Connection timed out", -4 => "Invalid message", -5 => "Not connected", _ => "Unknown error" };
                Ok(player.alloc_datum(Datum::String(text.to_owned())))
            }
            "getpeerconnectionlist" => Ok(player.alloc_datum(Datum::List(DatumType::List, VecDeque::new(), false))),
            "waitfornetconnection" => Ok(player.alloc_datum(Datum::Int(-1))),
            _ => Err(ScriptError::new(format!("No handler {} found for Multiuser xtra instance #{}", handler_name, instance_id))),
        }
    }

    pub fn create_instance(&mut self, _: &Vec<DatumRef>) -> u32 {
        self.instance_counter += 1;
        self.instances.insert(
            self.instance_counter,
            MultiuserXtraInstance {
                generation: self.generation_counter.wrapping_add(1),
                net_message_handler: None,
                message_queue: vec![],
                socket_tx: None,
                connection_mode: MultiuserConnectionMode::Binary,
                sender_id: String::new(),
                recv_buffer: Vec::new(),
                #[cfg(target_arch = "wasm32")]
                socket_resource: None,
                #[cfg(test)]
                teardown_probe: None,
            },
        );
        self.generation_counter = self.generation_counter.wrapping_add(1);
        self.instance_counter
    }

    pub fn has_instance_async_handler(_name: &str) -> bool {
        false
    }

    pub(crate) fn take_teardown_requests(
        &mut self,
    ) -> Vec<crate::player::xtra::manager::XtraTeardownRequest> {
        self.released_instances.drain(..).map(|released| {
                crate::player::xtra::manager::XtraTeardownRequest {
                    xtra_name: "Multiuser",
                    owner: self.owner.clone(),
                    instance_id: released.instance_id,
                    generation: released.generation,
                    #[cfg(target_arch = "wasm32")]
                    socket_resource: released.socket_resource,
                    #[cfg(test)]
                    drop_probe: released.teardown_probe,
                }
            })
            .collect()
    }

    pub(crate) fn rebind_owner(&mut self, owner: crate::player::ownership::OwnerToken) {
        self.owner = owner;
    }

    pub fn new() -> MultiuserXtraManager {
        Self::new_with_owner(crate::player::ownership::OwnerToken::transitional())
    }

    pub fn new_with_owner(owner: crate::player::ownership::OwnerToken) -> MultiuserXtraManager {
        MultiuserXtraManager {
            instances: FxHashMap::default(),
            instance_counter: 0,
            owner,
            generation_counter: 0,
            released_instances: Vec::new(),
        }
    }

    pub fn reset(&mut self) {
        for (id, mut instance) in self.instances.drain() {
            if !self.released_instances.iter().any(|released| {
                released.instance_id == id && released.generation == instance.generation
            }) {
                self.released_instances.push(ReleasedMultiuserInstance {
                    instance_id: id,
                    generation: instance.generation,
                    #[cfg(target_arch = "wasm32")]
                    socket_resource: instance.socket_resource.take(),
                    #[cfg(test)]
                    teardown_probe: instance.teardown_probe.take(),
                });
            }
        }
        self.instance_counter = 0;
    }
}


#[cfg(all(test, not(target_arch = "wasm32")))]
mod socket_event_tests {
    use super::{MultiuserConnectionMode, MultiuserSocketEvent, MultiuserXtraManager};
    use async_std::channel;
    use crate::director::static_datum::StaticDatum;
    use crate::player::ownership::{OwnerKey, OwnerToken};
    use crate::player::symbols::{builtin::BuiltInSymbol, symbol::Symbol};

    #[test]
    fn stale_and_removed_events_are_discarded_without_mutating_current_instance() {
        let owner = OwnerToken::new(OwnerKey { session: 41, player: 1, generation: 1 });
        let mut manager = MultiuserXtraManager::new_with_owner(owner.clone());
        let (queue, queued) = channel::unbounded();
        let instance_id = manager.create_instance(&Vec::new());
        let generation = manager.instance_generation(instance_id).expect("instance generation");
        manager.instances.get_mut(&instance_id).unwrap().connection_mode = MultiuserConnectionMode::Text;

        manager
            .handle_socket_event(
                instance_id,
                generation,
                MultiuserSocketEvent::Message(b"current".to_vec()),
                &queue,
                &owner,
            )
            .expect("current socket event should be handled");
        assert_eq!(manager.instances[&instance_id].message_queue.len(), 1);
        assert!(matches!(
            &manager.instances[&instance_id].message_queue[0].content,
            StaticDatum::String(value) if value == "current"
        ));
        assert!(queued.try_recv().is_err(), "an event without a handler must not enqueue a callback");
        manager.instances.get_mut(&instance_id).unwrap().net_message_handler =
            Some((crate::player::DatumRef::Void, Symbol::builtin(BuiltInSymbol::Nothing)));

        manager
            .handle_socket_event(
                instance_id,
                generation,
                MultiuserSocketEvent::Error("current error".to_owned()),
                &queue,
                &owner,
            )
            .expect("current socket error should be handled");
        assert_eq!(manager.instances[&instance_id].message_queue.len(), 2);
        assert!(matches!(
            &manager.instances[&instance_id].message_queue[1].content,
            StaticDatum::String(value) if value == "current error"
        ));
        assert!(queued.try_recv().is_ok(), "current event should enqueue its callback");

        manager
            .handle_socket_event(
                instance_id,
                generation.wrapping_add(1),
                MultiuserSocketEvent::Error("stale".to_owned()),
                &queue,
                &owner,
            )
            .expect("stale socket event should be discarded");
        assert_eq!(manager.instances[&instance_id].message_queue.len(), 2);
        assert!(queued.try_recv().is_err(), "stale event must not enqueue a callback");

        manager.instances.remove(&instance_id);
        manager
            .handle_socket_event(
                instance_id,
                generation,
                MultiuserSocketEvent::Error("removed".to_owned()),
                &queue,
                &owner,
            )
            .expect("removed-instance socket event should be discarded");
        assert!(!manager.instances.contains_key(&instance_id));
        assert!(queued.try_recv().is_err(), "removed event must not enqueue a callback");
    }
}
