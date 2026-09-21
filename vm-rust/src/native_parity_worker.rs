//! Standalone JSON-lines worker for the qualified native Director fixture path.
//!
//! This module intentionally owns a small copy of the parity wire schema. The
//! executable is consumed by another checkout and must not depend on that
//! checkout's Cargo workspace or game adapters.

use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    io::{self, Read, Write},
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
};

#[cfg(not(target_arch = "wasm32"))]
use std::{cell::RefCell, rc::Rc};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use url::Url;

use crate::director::file::{DirectorFile, read_director_file_bytes};
use crate::native_bevy_host::{HostOperation, NativeBevyHost};
#[cfg(not(target_arch = "wasm32"))]
use crate::player::session::NativeFlashCallbackObservation;
use crate::player::testing::{
    NativeGlobalReadError, NativeGlobalValue, NativeInvokeArgument, NativeInvokeError, TestPlayer,
};
#[cfg(all(test, not(target_arch = "wasm32")))]
use crate::player::testing::TestPlayerAdmission;
#[cfg(not(target_arch = "wasm32"))]
use crate::rendering::NativePresentationViewport;

const PROTOCOL_VERSION: u16 = 1;
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
const MAX_ADVANCE_US: u64 = 60_000_000;
const MAX_GLOBAL_IDENTIFIER_BYTES: usize = 128;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestEnvelope {
    version: u16,
    request_id: u64,
    #[serde(flatten)]
    request: Request,
}

#[derive(Debug, Serialize)]
struct ResponseEnvelope {
    version: u16,
    request_id: u64,
    #[serde(flatten)]
    response: Response,
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "op",
    content = "args",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(crate) enum Request {
    Discover,
    Start {
        config: Value,
    },
    Reset,
    Inspect {
        target: Target,
        path: String,
    },
    Modify {
        target: Target,
        path: String,
        value: Value,
    },
    Invoke {
        target: Target,
        function: String,
        arguments: Vec<Value>,
    },
    Input {
        event: VirtualInput,
    },
    Advance {
        duration_us: u64,
    },
    Capture {
        kind: CaptureKind,
    },
    Shutdown,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(crate) enum Response {
    Capabilities(Capabilities),
    Started { session: SessionHandle },
    Observation(Value),
    Invoked(Value),
    Captured(Capture),
    Acknowledged,
    Error(StructuredError),
}

#[derive(Debug, Serialize)]
struct Capabilities {
    runtime: String,
    runtime_version: String,
    operations: Vec<Capability>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum Capability {
    StateInspection,
    VirtualInput,
    ControlledTime,
    RgbaCapture,
    PcmCapture,
    Invocation,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Target {
    Root,
    Object(ObjectHandle),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ObjectHandle {
    session_id: u64,
    generation: u64,
    object_id: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CaptureKind {
    Rgba,
    Pcm,
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(crate) enum VirtualInput {
    Pointer {
        space: PointerSpace,
        x: f32,
        y: f32,
    },
    Button {
        button: MouseButton,
        state: ButtonState,
    },
    Key {
        key: String,
        state: ButtonState,
    },
    Focus {
        focused: bool,
    },
    Leave,
    Resize {
        width_physical: u32,
        height_physical: u32,
        scale: f32,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PointerSpace {
    Stage,
    Window,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ButtonState {
    Down,
    Up,
}

#[derive(Debug, Serialize)]
pub(crate) struct SessionHandle {
    id: u64,
    generation: u64,
}

#[derive(Debug, Serialize)]
pub(crate) struct StructuredError {
    code: ErrorCode,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ErrorCode {
    Unsupported,
    InvalidRequest,
    InvalidHandle,
    Runtime,
    NotReady,
    Protocol,
    Internal,
}

#[derive(Debug, Serialize)]
struct RgbaCapture {
    timestamp_us: u64,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    sha256: String,
}

#[derive(Debug, Serialize)]
struct PcmCapture {
    timestamp_us: u64,
    sample_rate: u32,
    channels: u16,
    samples: Vec<[f32; 2]>,
    sha256: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
enum Capture {
    Rgba(RgbaCapture),
    Pcm(PcmCapture),
}

fn pcm_sha256(samples: &[[f32; 2]]) -> String {
    let mut hasher = Sha256::new();
    for frame in samples {
        hasher.update(frame[0].to_le_bytes());
        hasher.update(frame[1].to_le_bytes());
    }
    format!("{:x}", hasher.finalize())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StartConfig {
    pub(crate) adapter: AdapterConfig,
    pub(crate) seed: u64,
    pub(crate) clock: ClockConfig,
    pub(crate) loading: LoadingConfig,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdapterConfig {
    pub(crate) backend: String,
    pub(crate) resource_root: String,
    pub(crate) movie: String,
    pub(crate) source_dcr_sha256: String,
    pub(crate) loading_policy: String,
    pub(crate) resource_aliases: Option<BTreeMap<String, ExternalCastAlias>>,
    #[serde(default)]
    pub(crate) presentation_viewport: Option<PresentationViewportConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PresentationViewportConfig {
    pub(crate) x: i64,
    pub(crate) y: i64,
    pub(crate) width: i64,
    pub(crate) height: i64,
}

impl PresentationViewportConfig {
    fn native(&self) -> NativePresentationViewport {
        NativePresentationViewport {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExternalCastAlias {
    path: String,
    sha256: String,
}

/// Opaque proof that all external cast declarations and their local resources
/// were validated by the native worker before a player was constructed.
#[derive(Debug)]
pub(crate) struct QualifiedExternalCasts {
    entries: BTreeMap<String, QualifiedExternalCast>,
}

#[derive(Debug)]
struct QualifiedExternalCast {
    requested_url: String,
    bytes: Vec<u8>,
}

impl QualifiedExternalCasts {
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn entry_for_request(&self, requested_url: &str) -> Option<(&str, &[u8])> {
        self.entries
            .iter()
            .find(|(_, entry)| entry.requested_url == requested_url)
            .map(|(key, entry)| (key.as_str(), entry.bytes.as_slice()))
    }

    pub(crate) fn from_entries(
        entries: impl IntoIterator<Item = (String, String, Vec<u8>)>,
    ) -> Self {
        Self {
            entries: entries
                .into_iter()
                .map(|(key, requested_url, bytes)| {
                    (
                        key,
                        QualifiedExternalCast {
                            requested_url,
                            bytes,
                        },
                    )
                })
                .collect(),
        }
    }

    #[cfg(test)]
    pub(crate) fn from_test_entries(
        entries: impl IntoIterator<Item = (String, String, Vec<u8>)>,
    ) -> Self {
        Self::from_entries(entries)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ClockConfig {
    pub(crate) epoch_us: i64,
    pub(crate) frame_rate_num: u32,
    pub(crate) frame_rate_den: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LoadingConfig {
    pub(crate) fixture: String,
}

pub(crate) struct Worker {
    pub(crate) player: Option<TestPlayer>,
    pub(crate) elapsed_us: u64,
    pub(crate) pointer: Option<(i32, i32)>,
    pub(crate) session: SessionHandle,
    pub(crate) shutdown: bool,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) callback_observer: Option<Rc<RefCell<Vec<NativeFlashCallbackObservation>>>>,
    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) start_failure_witness: Option<crate::player::testing::NativeTeardownWitness>,
    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) test_admission: Option<Rc<TestPlayerAdmission>>,
}

fn request(host: &mut NativeBevyHost, request: Request) -> Result<Response, StructuredError> {
    match request {
        Request::Discover => Ok(Response::Capabilities(Capabilities {
            runtime: "dirplayer-native".to_owned(),
            runtime_version: env!("CARGO_PKG_VERSION").to_owned(),
            operations: vec![
                Capability::StateInspection,
                Capability::VirtualInput,
                Capability::ControlledTime,
                Capability::RgbaCapture,
                Capability::PcmCapture,
                Capability::Invocation,
            ],
        })),
        Request::Start { config } => {
            if host.is_started() {
                return Err(error(
                    ErrorCode::InvalidRequest,
                    "one native worker process owns one scenario",
                ));
            }
            validate_parent_storage()?;
            let config: StartConfig = serde_json::from_value(config).map_err(|parse_error| {
                error_with(
                    ErrorCode::InvalidRequest,
                    format!("invalid deterministic start configuration: {parse_error}"),
                    None,
                )
            })?;
            host.submit(HostOperation::Start(config))
        }
        Request::Reset | Request::Modify { .. } => Err(unsupported(
            "native worker does not implement reset or mutation",
        )),
        Request::Invoke {
            target,
            function,
            arguments,
        } => {
            require_started(host)?;
            if !matches!(target, Target::Root) {
                return Err(error(
                    ErrorCode::InvalidHandle,
                    "native invocation accepts only the current root owner",
                ));
            }
            validate_global_identifier(&function)?;
            let arguments = arguments
                .into_iter()
                .enumerate()
                .map(|(index, value)| native_invoke_argument(index, value))
                .collect::<Result<Vec<_>, _>>()?;
            host.submit(HostOperation::Invoke {
                function,
                arguments,
            })
        }
        Request::Inspect { target, path } => {
            require_started(host)?;
            if !matches!(target, Target::Root) {
                return Err(error(
                    ErrorCode::InvalidHandle,
                    "native inspection accepts only the root target",
                ));
            }
            if let Some(name) = path.strip_prefix("globals.") {
                validate_global_identifier(name)?;
            }
            host.submit(HostOperation::Inspect(path))
        }
        Request::Input { event } => {
            require_started(host)?;
            validate_input_form(&event)?;
            host.submit(HostOperation::Input(event))
        }
        Request::Advance { duration_us } => {
            require_started(host)?;
            host.validate_advance(duration_us)?;
            host.submit(HostOperation::Advance(duration_us))
        }
        Request::Capture { kind } => {
            require_started(host)?;
            host.submit(HostOperation::Capture(kind))
        }
        Request::Shutdown => host.submit(HostOperation::Shutdown),
    }
}

fn require_started(host: &mut NativeBevyHost) -> Result<(), StructuredError> {
    if host.is_started() {
        Ok(())
    } else {
        Err(not_ready())
    }
}

fn validate_input_form(event: &VirtualInput) -> Result<(), StructuredError> {
    match event {
        VirtualInput::Pointer {
            space: PointerSpace::Stage,
            x,
            y,
        } => {
            checked_coordinate(*x, "x")?;
            checked_coordinate(*y, "y")?;
            Ok(())
        }
        VirtualInput::Pointer {
            space: PointerSpace::Window,
            ..
        } => Err(unsupported("window-space pointer input is not qualified")),
        VirtualInput::Button {
            button: MouseButton::Left,
            state: ButtonState::Down | ButtonState::Up,
        } => Ok(()),
        VirtualInput::Button { .. } => Err(unsupported("only left button down/up is qualified")),
        VirtualInput::Key { .. } => Err(unsupported("keyboard input is not qualified")),
        VirtualInput::Focus { .. } | VirtualInput::Leave | VirtualInput::Resize { .. } => {
            Err(unsupported("this virtual input form is not qualified"))
        }
    }
}

impl Worker {
    pub(crate) fn new() -> Self {
        Self {
            player: None,
            elapsed_us: 0,
            pointer: None,
            session: SessionHandle {
                id: 1,
                generation: 1,
            },
            shutdown: false,
            #[cfg(not(target_arch = "wasm32"))]
            callback_observer: None,
            #[cfg(all(test, not(target_arch = "wasm32")))]
            start_failure_witness: None,
            #[cfg(all(test, not(target_arch = "wasm32")))]
            test_admission: None,
        }
    }

    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) fn new_with_test_admission(
        admission: Rc<TestPlayerAdmission>,
    ) -> Self {
        let mut worker = Self::new();
        worker.test_admission = Some(admission);
        worker
    }

    pub(crate) fn start(&mut self, config: StartConfig) -> Result<Response, StructuredError> {
        if self.player.is_some() {
            return Err(error(
                ErrorCode::InvalidRequest,
                "one native worker process owns one scenario",
            ));
        }
        let seed = u32::try_from(config.seed).map_err(|_| {
            error(
                ErrorCode::InvalidRequest,
                "seed must be an unsigned 32-bit integer",
            )
        })?;
        if config.adapter.backend != "dirplayer-native" {
            return Err(error(
                ErrorCode::InvalidRequest,
                "adapter.backend must be dirplayer-native",
            ));
        }
        if config.adapter.loading_policy != "single-dcr" {
            return Err(error(
                ErrorCode::InvalidRequest,
                "adapter.loading_policy must be single-dcr",
            ));
        }
        if config.adapter.resource_root.trim().is_empty()
            || config.adapter.movie.trim().is_empty()
            || config.loading.fixture.trim().is_empty()
        {
            return Err(error(
                ErrorCode::InvalidRequest,
                "resource_root, movie, and loading.fixture must be nonempty",
            ));
        }
        if config.adapter.movie.starts_with('/') || Path::new(&config.adapter.movie).is_absolute() {
            return Err(error(
                ErrorCode::InvalidRequest,
                "adapter.movie must be a relative path",
            ));
        }
        if config.adapter.source_dcr_sha256.len() != 64
            || !config
                .adapter
                .source_dcr_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(error(
                ErrorCode::InvalidRequest,
                "source_dcr_sha256 must be 64 lowercase hexadecimal characters",
            ));
        }
        if config.clock.epoch_us != 0 {
            return Err(error(
                ErrorCode::Unsupported,
                "only epoch_us=0 is qualified",
            ));
        }
        if config.clock.frame_rate_den != 1 || config.clock.frame_rate_num == 0 {
            return Err(error(
                ErrorCode::InvalidRequest,
                "clock must have a positive frame_rate_num and frame_rate_den=1",
            ));
        }
        let movie_path = resolve_movie_path(&config.adapter.resource_root, &config.adapter.movie)?;
        let bytes = std::fs::read(&movie_path).map_err(|error| {
            error_with(
                ErrorCode::InvalidRequest,
                format!("cannot read configured movie: {error}"),
                None,
            )
        })?;
        let actual_hash = hex_sha256(&bytes);
        if actual_hash != config.adapter.source_dcr_sha256 {
            return Err(error_with(
                ErrorCode::InvalidRequest,
                "configured movie SHA-256 does not match source_dcr_sha256",
                Some(
                    json!({"expected":config.adapter.source_dcr_sha256,"actual":actual_hash,"movie":movie_path}),
                ),
            ));
        }

        let qualified_external_casts = config
            .adapter
            .resource_aliases
            .as_ref()
            .map(|aliases| {
                qualify_external_casts(
                    &canonical_resource_root(&config.adapter.resource_root)?,
                    &movie_path,
                    &bytes,
                    aliases,
                )
                .map_err(|message| error(ErrorCode::InvalidRequest, message))
            })
            .transpose()?;

        #[cfg(all(test, not(target_arch = "wasm32")))]
        let mut player = if let Some(admission) = self.test_admission.as_ref() {
            TestPlayer::try_new_with_test_admission(admission.clone())
        } else {
            TestPlayer::try_new()
        }
        .map_err(runtime_error)?;
        #[cfg(any(not(test), target_arch = "wasm32"))]
        let mut player = TestPlayer::try_new().map_err(runtime_error)?;
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(observer) = self.callback_observer.as_ref() {
            player.install_native_flash_callback_observer(observer.clone());
        }
        #[cfg(all(test, not(target_arch = "wasm32")))]
        let start_failure_witness = player.native_teardown_witness();
        player.set_deterministic_seed(seed).map_err(runtime_error)?;
        let movie_path_string = movie_path.to_str().ok_or_else(|| {
            error(
                ErrorCode::InvalidRequest,
                "configured movie path is not valid UTF-8",
            )
        })?;
        let load_result = if let Some(qualification) = qualified_external_casts.as_ref() {
            async_std::task::block_on(player.load_movie_quiet_with_qualified_casts(
                movie_path_string,
                bytes,
                qualification,
            ))
        } else {
            async_std::task::block_on(player.load_movie_quiet(movie_path_string, bytes))
        }
        .map_err(|load_error| match load_error {
            crate::player::testing::NativeMovieLoadError::Unsupported(message) => {
                unsupported(message)
            }
            crate::player::testing::NativeMovieLoadError::Runtime(error) => runtime_error(error),
        });
        load_result?;
        if let Some(viewport) = config.adapter.presentation_viewport.as_ref() {
            player
                .configure_native_presentation(Some(viewport.native()))
                .map_err(runtime_error)?;
        }
        let tempo = player.effective_tempo_quiet().map_err(runtime_error)?;
        if tempo != config.clock.frame_rate_num {
            #[cfg(all(test, not(target_arch = "wasm32")))]
            {
                self.start_failure_witness = Some(start_failure_witness);
            }
            return Err(error_with(
                ErrorCode::InvalidRequest,
                "configured frame rate does not match the loaded Director movie tempo",
                Some(json!({"configured":config.clock.frame_rate_num,"movie_tempo":tempo})),
            ));
        }
        async_std::task::block_on(player.try_init_movie_at_us(0)).map_err(runtime_error)?;
        self.player = Some(player);
        self.elapsed_us = 0;
        self.pointer = None;
        Ok(Response::Started {
            session: SessionHandle {
                id: 1,
                generation: 1,
            },
        })
    }

    pub(crate) fn inspect(&self, path: &str) -> Result<Response, StructuredError> {
        let Some(player) = self.player.as_ref() else {
            return Err(not_ready());
        };
        let value = match path {
            "current_frame" => json!(player.current_frame_quiet()),
            "current_label" => player
                .current_label_quiet()
                .map_or(Value::Null, |label| json!(label)),
            "simulation_time_us" => json!(self.elapsed_us),
            "init_state" => {
                let state = player.native_init_state_quiet().map_err(runtime_error)?;
                json!({
                    "session": self.session,
                    "next_frame": state.next_frame,
                    "flash_bindings": state
                        .flash_bindings
                        .into_iter()
                        .map(|binding| {
                            json!({
                                "sprite": binding.sprite,
                                "generation": binding.generation,
                                "cast_lib": binding.cast_lib,
                                "cast_member": binding.cast_member,
                                "asserted_frame": binding.asserted_frame,
                            })
                        })
                        .collect::<Vec<_>>(),
                    "pending_flash_actions": state
                        .pending_flash_actions
                        .into_iter()
                        .map(|action| {
                            json!({
                                "kind": action.kind,
                                "sprite": action.sprite,
                                "generation": action.generation,
                                "cast_lib": action.cast_lib,
                                "cast_member": action.cast_member,
                            })
                        })
                        .collect::<Vec<_>>(),
                })
            }
            "flash_instances" => {
                let snapshots = player
                    .native_flash_snapshots_quiet()
                    .map_err(runtime_error)?;
                json!({
                    "session": self.session,
                    "instances": snapshots
                        .into_iter()
                        .map(|snapshot| {
                            json!({
                                "sprite": snapshot.sprite,
                                "generation": snapshot.generation,
                                "width": snapshot.width,
                                "height": snapshot.height,
                                "current_frame": snapshot.current_frame,
                            })
                        })
                        .collect::<Vec<_>>(),
                })
            }
            "input_state" => {
                let state = player.native_input_state_quiet().map_err(runtime_error)?;
                let sprite_json = |sprite: crate::player::testing::NativeInputSpriteObservation| {
                    json!({
                        "sprite": sprite.sprite,
                        "member_ref": sprite.member_ref,
                        "member_name": sprite.member_name,
                    })
                };
                json!({
                    "session": self.session,
                    "owner": {
                        "session": state.owner.session,
                        "player": state.owner.player,
                        "generation": state.owner.generation,
                    },
                    "pointer": state.pointer.map(|(x, y)| json!({"x": x, "y": y})),
                    "mouse_down": state.mouse_down,
                    "captured_sprite": state.captured_sprite.map(&sprite_json),
                    "click_on_sprite": state.click_on_sprite.map(&sprite_json),
                    "hovered_sprites": state.hovered_sprites.into_iter().map(sprite_json).collect::<Vec<_>>(),
                })
            }
            _ if path.starts_with("globals") => {
                let name = path.strip_prefix("globals.").ok_or_else(|| {
                    error(
                        ErrorCode::InvalidRequest,
                        "global inspection paths must use globals.<identifier>",
                    )
                })?;
                validate_global_identifier(name)?;
                match player.native_global_value_quiet(name) {
                    Ok(Some(value)) => native_global_value_to_json(value)?,
                    Ok(None) => Value::Null,
                    Err(read_error) => return Err(native_global_read_error(read_error)),
                }
            }
            _ => {
                return Err(unsupported(
                    "native worker does not expose this inspection path",
                ));
            }
        };
        Ok(Response::Observation(value))
    }

    pub(crate) fn invoke(
        &self,
        function: String,
        arguments: Vec<NativeInvokeArgument>,
    ) -> Result<Response, StructuredError> {
        let player = self.player.as_ref().ok_or_else(not_ready)?;
        let value =
            async_std::task::block_on(player.native_invoke_global_quiet(&function, arguments))
                .map_err(native_invoke_error)?;
        Ok(Response::Invoked(native_global_value_to_json(value)?))
    }

    pub(crate) fn input(&mut self, event: VirtualInput) -> Result<Response, StructuredError> {
        match event {
            VirtualInput::Pointer {
                space: PointerSpace::Stage,
                x,
                y,
            } => {
                let x = checked_coordinate(x, "x")?;
                let y = checked_coordinate(y, "y")?;
                self.pointer = Some((x, y));
                let Some(player) = self.player.as_ref() else {
                    return Err(not_ready());
                };
                async_std::task::block_on(player.try_native_mouse_move(x, y))
                    .map_err(runtime_error)?;
                Ok(Response::Acknowledged)
            }
            VirtualInput::Pointer {
                space: PointerSpace::Window,
                ..
            } => Err(unsupported("window-space pointer input is not qualified")),
            VirtualInput::Button {
                button: MouseButton::Left,
                state: ButtonState::Down,
            } => {
                let (x, y) = self.pointer.ok_or_else(|| {
                    error(
                        ErrorCode::InvalidRequest,
                        "left button down requires a prior stage pointer",
                    )
                })?;
                let Some(player) = self.player.as_ref() else {
                    return Err(not_ready());
                };
                async_std::task::block_on(player.try_native_mouse_down(x, y))
                    .map_err(runtime_error)?;
                Ok(Response::Acknowledged)
            }
            VirtualInput::Button {
                button: MouseButton::Left,
                state: ButtonState::Up,
            } => {
                let (x, y) = self.pointer.ok_or_else(|| {
                    error(
                        ErrorCode::InvalidRequest,
                        "left button up requires a prior stage pointer",
                    )
                })?;
                let Some(player) = self.player.as_ref() else {
                    return Err(not_ready());
                };
                async_std::task::block_on(player.try_native_mouse_up(x, y))
                    .map_err(runtime_error)?;
                Ok(Response::Acknowledged)
            }
            VirtualInput::Button { .. } => {
                Err(unsupported("only left button down/up is qualified"))
            }
            VirtualInput::Key { .. } => Err(unsupported("keyboard input is not qualified")),
            VirtualInput::Focus { .. } | VirtualInput::Leave | VirtualInput::Resize { .. } => {
                Err(unsupported("this virtual input form is not qualified"))
            }
        }
    }

    pub(crate) fn validate_advance(&self, duration_us: u64) -> Result<(), StructuredError> {
        if duration_us > MAX_ADVANCE_US {
            return Err(error(
                ErrorCode::InvalidRequest,
                "advance exceeds the 60 second deterministic step budget",
            ));
        }
        let target = self
            .elapsed_us
            .checked_add(duration_us)
            .ok_or_else(|| error(ErrorCode::InvalidRequest, "simulation time overflow"))?;
        let _ = target;
        Ok(())
    }

    pub(crate) fn advance(&mut self, duration_us: u64) -> Result<Response, StructuredError> {
        self.validate_advance(duration_us)?;
        let target = self
            .elapsed_us
            .checked_add(duration_us)
            .ok_or_else(|| error(ErrorCode::InvalidRequest, "simulation time overflow"))?;
        let Some(player) = self.player.as_mut() else {
            return Err(not_ready());
        };
        async_std::task::block_on(player.try_advance_to_us(target)).map_err(runtime_error)?;
        self.elapsed_us = target;
        Ok(Response::Acknowledged)
    }

    pub(crate) fn capture(&self) -> Result<Response, StructuredError> {
        let Some(player) = self.player.as_ref() else {
            return Err(not_ready());
        };
        let snapshot = player.snapshot_rgba_quiet().map_err(runtime_error)?;
        let mut hasher = Sha256::new();
        hasher.update(&snapshot.data);
        Ok(Response::Captured(Capture::Rgba(RgbaCapture {
            timestamp_us: self.elapsed_us,
            width: snapshot.width,
            height: snapshot.height,
            sha256: format!("{:x}", hasher.finalize()),
            pixels: snapshot.data,
        })))
    }

    pub(crate) fn pcm_capture(&self) -> Result<Response, StructuredError> {
        let Some(player) = self.player.as_ref() else {
            return Err(not_ready());
        };
        let samples = player.native_pcm_snapshot_quiet();
        if samples.is_empty() {
            return Err(unsupported(
                "native PCM capture is empty; advance the simulation first",
            ));
        }
        let sha256 = pcm_sha256(&samples);
        Ok(Response::Captured(Capture::Pcm(PcmCapture {
            timestamp_us: 0,
            sample_rate: 48_000,
            channels: 2,
            samples,
            sha256,
        })))
    }

    pub(crate) fn shutdown(&mut self) -> Result<Response, StructuredError> {
        self.player.take();
        self.shutdown = true;
        Ok(Response::Acknowledged)
    }
}

fn run_with_io(
    input: &mut impl Read,
    output: &mut impl Write,
    host: &mut NativeBevyHost,
) -> io::Result<()> {
    let loop_result = (|| -> io::Result<()> {
        loop {
            let Some(line) = read_bounded_line(input)? else {
                break;
            };
            if line.len() > MAX_LINE_BYTES {
                write_response(
                    output,
                    0,
                    Response::Error(error(
                        ErrorCode::Protocol,
                        format!("request exceeds {MAX_LINE_BYTES} byte limit"),
                    )),
                )?;
                continue;
            }
            let envelope = match serde_json::from_slice::<RequestEnvelope>(&line) {
                Ok(envelope) => envelope,
                Err(parse_error) => {
                    write_response(
                        output,
                        0,
                        Response::Error(error(
                            ErrorCode::Protocol,
                            format!("invalid request JSON: {parse_error}"),
                        )),
                    )?;
                    continue;
                }
            };
            let response = if envelope.version != PROTOCOL_VERSION {
                Response::Error(error(
                    ErrorCode::Protocol,
                    format!("unsupported protocol version {}", envelope.version),
                ))
            } else {
                request(host, envelope.request).unwrap_or_else(Response::Error)
            };
            write_response(output, envelope.request_id, response)?;
            if host.is_shutdown() {
                break;
            }
        }
        Ok(())
    })();

    let retire_result = if host.is_shutdown() {
        Ok(())
    } else {
        host.submit(HostOperation::Shutdown)
            .map(|_| ())
            .map_err(|error| io::Error::other(error.message))
    };

    match loop_result {
        Err(error) => Err(error),
        Ok(()) => retire_result,
    }
}

pub fn run() -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdin = stdin.lock();
    let stdout = io::stdout();
    let mut stdout = io::BufWriter::new(stdout.lock());
    let mut host = NativeBevyHost::new();
    run_with_io(&mut stdin, &mut stdout, &mut host)
}

fn write_response(output: &mut impl Write, request_id: u64, response: Response) -> io::Result<()> {
    let envelope = ResponseEnvelope {
        version: PROTOCOL_VERSION,
        request_id,
        response,
    };
    let mut bytes = serde_json::to_vec(&envelope).map_err(io::Error::other)?;
    bytes.push(b'\n');
    if bytes.len() > MAX_LINE_BYTES {
        return Err(io::Error::other(
            "response exceeds bounded JSON-lines limit",
        ));
    }
    output.write_all(&bytes)?;
    output.flush()
}

fn read_bounded_line(input: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        let count = input.read(&mut byte)?;
        if count == 0 {
            return if line.is_empty() {
                Ok(None)
            } else {
                Ok(Some(line))
            };
        }
        line.push(byte[0]);
        if line.len() > MAX_LINE_BYTES {
            while byte[0] != b'\n' {
                if input.read(&mut byte)? == 0 {
                    break;
                }
            }
            return Ok(Some(vec![0; MAX_LINE_BYTES + 1]));
        }
        if byte[0] == b'\n' {
            return Ok(Some(line));
        }
    }
}

fn validate_parent_storage() -> Result<(), StructuredError> {
    for name in ["PARITY_SESSION_ROOT", "TMPDIR"] {
        let value = std::env::var_os(name).ok_or_else(|| {
            error(
                ErrorCode::InvalidRequest,
                format!("{name} must be supplied by the parent"),
            )
        })?;
        let path = PathBuf::from(value);
        if !path.is_dir() {
            return Err(error(
                ErrorCode::InvalidRequest,
                format!("{name} must name an existing directory"),
            ));
        }
    }
    Ok(())
}

fn canonical_resource_root(root: &str) -> Result<PathBuf, StructuredError> {
    let root = PathBuf::from(root);
    let root = std::fs::canonicalize(if root.is_absolute() {
        root
    } else {
        std::env::current_dir()
            .map_err(|error| {
                error_with(
                    ErrorCode::Runtime,
                    format!("cannot resolve current directory: {error}"),
                    None,
                )
            })?
            .join(root)
    })
    .map_err(|error| {
        error_with(
            ErrorCode::InvalidRequest,
            format!("resource_root is not a readable directory: {error}"),
            None,
        )
    })?;
    if !root.is_dir() {
        return Err(error(
            ErrorCode::InvalidRequest,
            "resource_root must be a directory",
        ));
    }
    Ok(root)
}

fn resolve_movie_path(root: &str, movie: &str) -> Result<PathBuf, StructuredError> {
    let root = canonical_resource_root(root)?;
    let movie = std::fs::canonicalize(root.join(movie)).map_err(|error| {
        error_with(
            ErrorCode::InvalidRequest,
            format!("configured movie is not readable: {error}"),
            None,
        )
    })?;
    if !movie.starts_with(&root) || !movie.is_file() {
        return Err(error(
            ErrorCode::InvalidRequest,
            "configured movie must remain beneath resource_root and be a regular file",
        ));
    }
    Ok(movie)
}

fn qualify_external_casts(
    resource_root: &Path,
    movie_path: &Path,
    movie_bytes: &[u8],
    aliases: &BTreeMap<String, ExternalCastAlias>,
) -> Result<QualifiedExternalCasts, String> {
    let movie_name = movie_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "configured movie path is not valid UTF-8".to_owned())?;
    let movie_dir = movie_path
        .parent()
        .ok_or_else(|| "configured movie path has no parent".to_owned())?;
    let movie_base_url = Url::from_directory_path(movie_dir)
        .map_err(|_| "configured movie parent is not a file URL path".to_owned())?
        .to_string();
    let movie = parse_director_file(movie_bytes, movie_name, &movie_base_url)
        .map_err(|error| format!("configured movie is not a valid Director file: {error}"))?;

    let mut required = BTreeSet::new();
    for entry in movie
        .cast_entries
        .iter()
        .filter(|entry| !entry.file_path.is_empty())
    {
        let key = normalize_external_cast_key(&entry.file_path)?;
        if !required.insert(key.clone()) {
            return Err(format!(
                "external cast declarations normalize to duplicate alias key '{key}'"
            ));
        }
    }
    validate_alias_keys(&required, aliases)?;

    let root_base_url = Url::from_directory_path(resource_root)
        .map_err(|_| "resource_root is not a file URL path".to_owned())?
        .to_string();
    let movie_base = Url::parse(&movie_base_url)
        .map_err(|_| "configured movie parent is not a valid file URL".to_owned())?;
    let mut targets = HashSet::new();
    let mut entries = BTreeMap::new();
    for key in required {
        let alias = aliases
            .get(&key)
            .expect("exact alias-set validation must include every key");
        validate_sha256(&alias.sha256)?;
        let target = resolve_alias_path(resource_root, &alias.path)?;
        insert_unique_target(&mut targets, target.clone(), &key)?;
        let bytes = std::fs::read(&target)
            .map_err(|error| format!("cannot read resource alias '{key}': {error}"))?;
        validate_alias_hash(&key, &bytes, &alias.sha256)?;
        parse_director_file(&bytes, &key, &root_base_url).map_err(|error| {
            format!("resource alias '{key}' is not a valid Director cast: {error}")
        })?;
        let requested_url = movie_base
            .join(&key)
            .map_err(|_| format!("external cast alias '{key}' cannot form a request URL"))?
            .to_string();
        entries.insert(
            key,
            QualifiedExternalCast {
                requested_url,
                bytes,
            },
        );
    }
    Ok(QualifiedExternalCasts { entries })
}

fn parse_director_file(
    bytes: &[u8],
    file_name: &str,
    base_url: &str,
) -> Result<DirectorFile, String> {
    if !(bytes.starts_with(b"XFIR") || bytes.starts_with(b"RIFX")) {
        return Err("missing XFIR/RIFX Director signature".to_owned());
    }
    let owned = bytes.to_vec();
    catch_unwind(AssertUnwindSafe(|| {
        read_director_file_bytes(&owned, file_name, base_url)
    }))
    .map_err(|_| "Director parser panicked on malformed bytes".to_owned())?
}

fn validate_alias_keys(
    required: &BTreeSet<String>,
    aliases: &BTreeMap<String, ExternalCastAlias>,
) -> Result<(), String> {
    let supplied = aliases.keys().cloned().collect::<BTreeSet<_>>();
    if supplied == *required {
        return Ok(());
    }
    let missing = required.difference(&supplied).cloned().collect::<Vec<_>>();
    let extra = supplied.difference(required).cloned().collect::<Vec<_>>();
    Err(format!(
        "resource_aliases must exactly match declared external casts (missing: {:?}, extra: {:?})",
        missing, extra
    ))
}

fn normalize_external_cast_key(path: &str) -> Result<String, String> {
    let normalized = path.replace('\\', "/");
    let basename = normalized
        .split('/')
        .filter(|segment| !segment.is_empty())
        .last()
        .ok_or_else(|| "external cast declaration has no basename".to_owned())?;
    if basename == "." || basename == ".." || basename.contains('\0') {
        return Err("external cast declaration has an invalid basename".to_owned());
    }
    let key = match basename.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => format!("{stem}.cct"),
        _ => format!("{basename}.cct"),
    };
    Ok(key)
}

fn validate_sha256(value: &str) -> Result<(), String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err("resource alias sha256 must be 64 lowercase hexadecimal characters".to_owned());
    }
    Ok(())
}

fn validate_alias_hash(key: &str, bytes: &[u8], expected: &str) -> Result<(), String> {
    let actual = hex_sha256(bytes);
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "resource alias '{key}' SHA-256 mismatch: expected {expected}, got {actual}"
        ))
    }
}

fn resolve_alias_path(resource_root: &Path, raw: &str) -> Result<PathBuf, String> {
    let normalized = raw.replace('\\', "/");
    if normalized.is_empty()
        || normalized.starts_with('/')
        || normalized.starts_with("//")
        || normalized.as_bytes().get(1) == Some(&b':')
    {
        return Err(format!("resource alias path must be relative: '{raw}'"));
    }
    for component in normalized.split('/') {
        if component == ".." {
            return Err(format!(
                "resource alias path may not traverse parents: '{raw}'"
            ));
        }
        if component.contains(':') {
            return Err(format!(
                "resource alias path contains an unsafe drive or URI component: '{raw}'"
            ));
        }
    }
    let target = std::fs::canonicalize(resource_root.join(Path::new(&normalized)))
        .map_err(|error| format!("resource alias path is not readable: {error}"))?;
    if !target.starts_with(resource_root) || !target.is_file() {
        return Err(format!(
            "resource alias path must remain beneath resource_root and be a regular file: '{raw}'"
        ));
    }
    Ok(target)
}

fn insert_unique_target(
    targets: &mut HashSet<PathBuf>,
    target: PathBuf,
    key: &str,
) -> Result<(), String> {
    if targets.insert(target.clone()) {
        Ok(())
    } else {
        Err(format!(
            "resource_aliases contains duplicate target '{}': alias '{key}'",
            target.display()
        ))
    }
}

fn checked_coordinate(value: f32, name: &str) -> Result<i32, StructuredError> {
    let value_as_f64 = value as f64;
    if !value.is_finite()
        || value.fract() != 0.0
        || value_as_f64 < i32::MIN as f64
        || value_as_f64 > i32::MAX as f64
    {
        return Err(error(
            ErrorCode::InvalidRequest,
            format!("stage pointer {name} must be a finite integral i32"),
        ));
    }
    Ok(value as i32)
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn validate_global_identifier(identifier: &str) -> Result<(), StructuredError> {
    let bytes = identifier.as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_GLOBAL_IDENTIFIER_BYTES {
        return Err(error(
            ErrorCode::InvalidRequest,
            format!("global identifier must be 1..={MAX_GLOBAL_IDENTIFIER_BYTES} ASCII bytes"),
        ));
    }
    if !bytes[0].is_ascii_alphabetic() && bytes[0] != b'_' {
        return Err(error(
            ErrorCode::InvalidRequest,
            "global identifier must start with an ASCII letter or underscore",
        ));
    }
    if !bytes[1..]
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    {
        return Err(error(
            ErrorCode::InvalidRequest,
            "global identifier must contain only ASCII letters, digits, or underscores",
        ));
    }
    Ok(())
}

fn native_invoke_argument(
    index: usize,
    value: Value,
) -> Result<NativeInvokeArgument, StructuredError> {
    let invalid = |reason: &str| {
        error(
            ErrorCode::InvalidRequest,
            format!("invoke argument {index} {reason}"),
        )
    };
    match value {
        Value::Null => Ok(NativeInvokeArgument::Void),
        Value::Bool(value) => {
            // This is the established browser/protocol convention: Director
            // booleans are represented as integer 1/0 in Lingo datum values.
            Ok(NativeInvokeArgument::Int(i32::from(value)))
        }
        Value::String(value) => Ok(NativeInvokeArgument::String(value)),
        Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                let value = i32::try_from(value)
                    .map_err(|_| invalid("integer is outside the Director Int range"))?;
                Ok(NativeInvokeArgument::Int(value))
            } else {
                let value = value
                    .as_f64()
                    .filter(|value| value.is_finite())
                    .ok_or_else(|| invalid("number must be finite"))?;
                Ok(NativeInvokeArgument::Float(value))
            }
        }
        Value::Object(mut object) => {
            let kind = object
                .remove("type")
                .and_then(|value| value.as_str().map(str::to_owned))
                .ok_or_else(|| invalid("object must be a typed symbol"))?;
            if kind != "symbol" {
                return Err(invalid("object type is unsupported"));
            }
            let value = object
                .remove("value")
                .and_then(|value| value.as_str().map(str::to_owned))
                .ok_or_else(|| invalid("symbol value must be a string"))?;
            if !object.is_empty() {
                return Err(invalid("symbol object has unknown fields"));
            }
            Ok(NativeInvokeArgument::Symbol(value))
        }
        Value::Array(_) => Err(invalid("arrays are unsupported")),
    }
}

fn native_global_value_to_json(value: NativeGlobalValue) -> Result<Value, StructuredError> {
    Ok(match value {
        NativeGlobalValue::Int(value) => json!(value),
        NativeGlobalValue::Float(value) => json!(value),
        NativeGlobalValue::String(value) => json!(value),
        NativeGlobalValue::Symbol(value) => json!({"type": "symbol", "value": value}),
        NativeGlobalValue::Void => Value::Null,
        NativeGlobalValue::List(values) => Value::Array(
            values
                .into_iter()
                .map(native_global_value_to_json)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        NativeGlobalValue::PropList(values) => Value::Array(
            values
                .into_iter()
                .map(|(key, value)| {
                    Ok(json!([
                        native_global_value_to_json(key)?,
                        native_global_value_to_json(value)?
                    ]))
                })
                .collect::<Result<Vec<_>, StructuredError>>()?,
        ),
        NativeGlobalValue::Point(values) => json!(values),
        NativeGlobalValue::Rect(values) => json!(values),
    })
}

fn native_global_read_error(error: NativeGlobalReadError) -> StructuredError {
    native_value_read_error("global", error)
}

fn native_value_read_error(subject: &str, error: NativeGlobalReadError) -> StructuredError {
    match error {
        NativeGlobalReadError::Runtime(error) => runtime_error(error),
        NativeGlobalReadError::Unsupported { datum_type, reason } => error_with(
            ErrorCode::Unsupported,
            format!("{subject} datum type {datum_type} is not representable in v1"),
            Some(json!({"datum_type": datum_type, "reason": reason})),
        ),
    }
}

fn native_invoke_error(error: NativeInvokeError) -> StructuredError {
    match error {
        NativeInvokeError::Runtime(error) => runtime_error(error),
        NativeInvokeError::Returned(error) => native_value_read_error("returned", error),
        NativeInvokeError::UnsupportedContinuation(reason) => unsupported(reason),
    }
}

fn runtime_error(error: crate::player::ScriptError) -> StructuredError {
    error_with(ErrorCode::Runtime, error.message, None)
}

fn unsupported(message: impl Into<String>) -> StructuredError {
    error(ErrorCode::Unsupported, message)
}

fn not_ready() -> StructuredError {
    error(
        ErrorCode::NotReady,
        "native worker has not started a session",
    )
}

fn error(code: ErrorCode, message: impl Into<String>) -> StructuredError {
    error_with(code, message, None)
}

fn error_with(
    code: ErrorCode,
    message: impl Into<String>,
    details: Option<Value>,
) -> StructuredError {
    StructuredError {
        code,
        message: message.into(),
        details,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    #[cfg(not(target_arch = "wasm32"))]
    use std::io::{Cursor, ErrorKind};

    #[cfg(not(target_arch = "wasm32"))]
    const PROBE_MOVIE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/native_director_probe.dcr"
    );

    #[cfg(not(target_arch = "wasm32"))]
    fn initialized_probe_player() -> TestPlayer {
        let mut player = TestPlayer::try_new().expect("probe player construction failed");
        let bytes = std::fs::read(PROBE_MOVIE).expect("probe movie must be readable");
        async_std::task::block_on(player.load_movie_quiet(PROBE_MOVIE, bytes))
            .expect("probe movie must load");
        async_std::task::block_on(player.try_init_movie_at_us(0))
            .expect("probe movie must initialize");
        player
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn spybot_start_config_from_env() -> Option<StartConfig> {
        let resource_root = std::env::var("SPYBOT_RESOURCE_ROOT").ok()?;
        let resource_root_path = PathBuf::from(&resource_root);
        let movie = "spybot-nightfall-incident.dcr".to_owned();
        let _movie_bytes = std::fs::read(resource_root_path.join(&movie))
            .expect("SPYBOT_RESOURCE_ROOT must contain the Spybot DCR");
        let cast_sha256 = "bd18e2325e2074a24b86db5a921b873e1f55bd38db7a7646a57eb0d929006035";
        let casts = [
            (
                "sound_level_1.cct",
                "spybot-nightfall-incident-sound-level-1.cct",
            ),
            (
                "sound_level_2.cct",
                "spybot-nightfall-incident-sound-level-2.cct",
            ),
            (
                "sound_level_3.cct",
                "spybot-nightfall-incident-sound-level-3.cct",
            ),
            (
                "sound_level_4.cct",
                "spybot-nightfall-incident-sound-level-4.cct",
            ),
            (
                "sound_level_5.cct",
                "spybot-nightfall-incident-sound-level-5.cct",
            ),
        ];
        let resource_aliases = casts
            .into_iter()
            .map(|(key, path)| {
                (
                    key.to_owned(),
                    ExternalCastAlias {
                        path: path.to_owned(),
                        sha256: cast_sha256.to_owned(),
                    },
                )
            })
            .collect();
        Some(StartConfig {
            adapter: AdapterConfig {
                backend: "dirplayer-native".to_owned(),
                resource_root,
                movie,
                source_dcr_sha256:
                    "ddf24b667a8d014856d9db42e1658cbf9714847f1cadc2c2c14d21f8e5950c77".to_owned(),
                loading_policy: "single-dcr".to_owned(),
                resource_aliases: Some(resource_aliases),
                presentation_viewport: Some(PresentationViewportConfig {
                    x: 0,
                    y: 0,
                    width: 650,
                    height: 420,
                }),
            },
            seed: 0,
            clock: ClockConfig {
                epoch_us: 0,
                frame_rate_num: 20,
                frame_rate_den: 1,
            },
            loading: LoadingConfig {
                fixture: "spybot-title".to_owned(),
            },
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn assert_retired(witness: crate::player::testing::NativeTeardownWitness) {
        assert!(!witness.owner.is_arena_live());
        assert!(
            witness
                .session
                .borrow_mut()
                .with_player(witness.player_id, |_| ())
                .is_none()
        );
        assert!(
            witness
                .session
                .borrow()
                .native_presentation(witness.player_id)
                .is_none()
        );
        assert!(
            witness
                .session
                .borrow()
            .native_flash(witness.player_id)
            .is_none()
        );
        assert!(witness
            .session
            .borrow()
            .native_player_notification_sink(witness.player_id, &witness.owner)
            .is_none());
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn assert_live(witness: &crate::player::testing::NativeTeardownWitness) {
        assert!(witness.owner.is_arena_live());
        assert!(witness
            .session
            .borrow_mut()
            .with_player(witness.player_id, |_| ())
            .is_some());
        assert!(witness
            .session
            .borrow()
            .native_presentation(witness.player_id)
            .is_some());
        assert!(witness
            .session
            .borrow()
            .native_flash(witness.player_id)
            .is_some());
        assert!(witness
            .session
            .borrow()
            .native_player_notification_sink(witness.player_id, &witness.owner)
            .is_some());
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn response_value(response: Response) -> Value {
        serde_json::to_value(response).expect("native response must serialize")
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn pair_submit(
        first: &mut NativeBevyHost,
        second: &mut NativeBevyHost,
        operation: impl Fn() -> HostOperation,
    ) -> (Response, Response) {
        let first_result = first
            .submit(operation())
            .expect("first host operation must succeed");
        let second_result = second
            .submit(operation())
            .expect("second host operation must succeed");
        (first_result, second_result)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn pair_observation(
        first: &mut NativeBevyHost,
        second: &mut NativeBevyHost,
        path: &str,
    ) -> (Value, Value) {
        let (first_response, second_response) = pair_submit(first, second, || {
            HostOperation::Inspect(path.to_owned())
        });
        match (first_response, second_response) {
            (Response::Observation(first), Response::Observation(second)) => (first, second),
            responses => panic!("expected observations for {path}, got {responses:?}"),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn single_observation(host: &mut NativeBevyHost, path: &str) -> Value {
        match host
            .submit(HostOperation::Inspect(path.to_owned()))
            .expect("host observation must succeed")
        {
            Response::Observation(value) => value,
            response => panic!("expected observation for {path}, got {response:?}"),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn pair_capture(
        first: &mut NativeBevyHost,
        second: &mut NativeBevyHost,
        kind: CaptureKind,
    ) -> (Value, Value) {
        let (first_response, second_response) = pair_submit(first, second, || {
            HostOperation::Capture(match &kind {
                CaptureKind::Rgba => CaptureKind::Rgba,
                CaptureKind::Pcm => CaptureKind::Pcm,
            })
        });
        let first = response_value(first_response);
        let second = response_value(second_response);
        assert_eq!(first["kind"], json!("captured"));
        assert_eq!(second["kind"], json!("captured"));
        assert_eq!(first, second, "dual-host capture diverged");
        (first, second)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn single_capture(host: &mut NativeBevyHost, kind: CaptureKind) -> Value {
        response_value(
            host.submit(HostOperation::Capture(kind))
                .expect("host capture must succeed"),
        )
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn capture_metadata(value: &Value) -> Value {
        let capture = &value["value"]["value"];
        json!({
            "kind": value["value"]["kind"],
            "timestamp_us": capture["timestamp_us"],
            "frame_count": capture["samples"].as_array().map(Vec::len),
            "width": capture["width"],
            "height": capture["height"],
            "sha256": capture["sha256"],
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn assert_equal_input_state(
        first: &Value,
        second: &Value,
        first_owner: crate::player::ownership::OwnerKey,
        second_owner: crate::player::ownership::OwnerKey,
    ) {
        for (value, owner) in [(first, first_owner), (second, second_owner)] {
            assert_eq!(value["owner"]["session"], json!(owner.session));
            assert_eq!(value["owner"]["player"], json!(owner.player));
            assert_eq!(value["owner"]["generation"], json!(owner.generation));
        }
        for field in [
            "pointer",
            "mouse_down",
            "captured_sprite",
            "click_on_sprite",
            "hovered_sprites",
        ] {
            assert_eq!(
                first[field], second[field],
                "dual-host input state diverged in {field}"
            );
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    struct ReadFailure;

    #[cfg(not(target_arch = "wasm32"))]
    impl Read for ReadFailure {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(
                ErrorKind::Interrupted,
                "injected read failure",
            ))
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    struct WriteFailure;

    #[cfg(not(target_arch = "wasm32"))]
    impl Write for WriteFailure {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(
                ErrorKind::BrokenPipe,
                "injected write failure",
            ))
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::new(
                ErrorKind::BrokenPipe,
                "injected write failure",
            ))
        }
    }

    #[test]
    fn global_symbol_uses_typed_wire_shape() {
        assert_eq!(
            native_global_value_to_json(NativeGlobalValue::Symbol("Probe".to_owned())).unwrap(),
            json!({"type": "symbol", "value": "Probe"}),
        );
    }

    #[test]
    fn unsupported_global_conversion_preserves_stable_datum_type() {
        let error = native_global_read_error(NativeGlobalReadError::Unsupported {
            datum_type: "null",
            reason: None,
        });
        assert!(matches!(error.code, ErrorCode::Unsupported));
        assert_eq!(
            error.details,
            Some(json!({"datum_type": "null", "reason": null}))
        );
    }

    #[test]
    fn invoke_arguments_use_director_boolean_and_typed_symbol_conventions() {
        assert!(matches!(
            native_invoke_argument(0, json!(true)).unwrap(),
            NativeInvokeArgument::Int(1)
        ));
        assert!(matches!(
            native_invoke_argument(1, json!({"type": "symbol", "value": "snd_start"}))
                .unwrap(),
            NativeInvokeArgument::Symbol(value) if value == "snd_start"
        ));
        assert!(native_invoke_argument(2, json!([1, 2])).is_err());
    }

    #[test]
    fn returned_datum_errors_are_labeled_as_returned_values() {
        let error = native_invoke_error(NativeInvokeError::Returned(
            NativeGlobalReadError::Unsupported {
                datum_type: "sound_channel",
                reason: Some("wire_shape"),
            },
        ));
        assert!(
            error
                .message
                .starts_with("returned datum type sound_channel")
        );
        assert_eq!(
            error.details,
            Some(json!({"datum_type": "sound_channel", "reason": "wire_shape"}))
        );
    }

    #[test]
    fn pcm_hash_uses_little_endian_stereo_frames() {
        let frames = [[0.0_f32, 1.0], [0.25, -0.5]];
        assert_eq!(
            pcm_sha256(&frames),
            "a598f911bbd93c4319b851d86ba51db07e740f128e3d82eb72be86d2aa38e9de"
        );
    }

    #[test]
    fn normalizes_arbitrary_external_cast_declarations() {
        assert_eq!(
            normalize_external_cast_key(r"C:\phase\Alpha.CST").unwrap(),
            "Alpha.cct"
        );
        assert_eq!(
            normalize_external_cast_key("nested/beta").unwrap(),
            "beta.cct"
        );
        assert_eq!(
            normalize_external_cast_key("A/one.cst").unwrap(),
            normalize_external_cast_key(r"B\one.cst").unwrap()
        );
    }

    #[test]
    fn requires_exact_alias_keys_for_one_or_many_declarations() {
        let one = BTreeSet::from(["alpha.cct".to_owned()]);
        let one_alias = BTreeMap::from([(
            "alpha.cct".to_owned(),
            ExternalCastAlias {
                path: "alpha.cct".to_owned(),
                sha256: "0".repeat(64),
            },
        )]);
        assert!(validate_alias_keys(&one, &one_alias).is_ok());

        let five = BTreeSet::from([
            "a.cct".to_owned(),
            "b.cct".to_owned(),
            "c.cct".to_owned(),
            "d.cct".to_owned(),
            "e.cct".to_owned(),
        ]);
        assert!(validate_alias_keys(&five, &one_alias).is_err());
        let extra = BTreeMap::from([
            (
                "alpha.cct".to_owned(),
                ExternalCastAlias {
                    path: "alpha.cct".to_owned(),
                    sha256: "0".repeat(64),
                },
            ),
            (
                "extra.cct".to_owned(),
                ExternalCastAlias {
                    path: "extra.cct".to_owned(),
                    sha256: "0".repeat(64),
                },
            ),
        ]);
        assert!(validate_alias_keys(&one, &extra).is_err());
    }

    #[test]
    fn rejects_malformed_hash_and_hostile_paths() {
        assert!(validate_sha256(&"A".repeat(64)).is_err());
        assert!(validate_sha256("short").is_err());
        assert!(validate_alias_hash("valid.cct", b"actual", &hex_sha256(b"other")).is_err());
        let root = std::env::temp_dir().join(format!(
            "dirplayer-alias-path-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        std::fs::write(root.join("valid.cct"), b"valid").unwrap();
        std::fs::create_dir(root.join("directory.cct")).unwrap();
        for path in ["/absolute.cct", r"C:\absolute.cct", r"nested\..\escape.cct"] {
            assert!(resolve_alias_path(&root, path).is_err());
        }
        assert!(resolve_alias_path(&root, "missing.cct").is_err());
        assert!(resolve_alias_path(&root, "directory.cct").is_err());
        assert!(resolve_alias_path(&root, "valid.cct").is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn catches_signature_and_parser_failures_as_errors() {
        assert!(parse_director_file(b"not-a-cast", "bad.cct", "file:///tmp/").is_err());
        assert!(parse_director_file(b"XFIR malformed", "bad.cct", "file:///tmp/").is_err());
    }

    #[test]
    fn pre_start_requests_do_not_submit_host_operations() {
        let mut host = NativeBevyHost::new();
        let requests = [
            Request::Inspect {
                target: Target::Root,
                path: "current_frame".to_owned(),
            },
            Request::Invoke {
                target: Target::Root,
                function: "go".to_owned(),
                arguments: vec![],
            },
            Request::Input {
                event: VirtualInput::Pointer {
                    space: PointerSpace::Stage,
                    x: 1.0,
                    y: 1.0,
                },
            },
            Request::Advance { duration_us: 0 },
            Request::Capture {
                kind: CaptureKind::Rgba,
            },
        ];
        for operation in requests {
            assert!(matches!(
                request(&mut host, operation),
                Err(StructuredError {
                    code: ErrorCode::NotReady,
                    ..
                })
            ));
        }
        assert_eq!(host.test_elapsed_us(), 0);
        assert!(!host.test_has_pending_operation());
    }

    #[test]
    fn reset_and_modify_remain_explicitly_unsupported() {
        let mut host = NativeBevyHost::new();
        assert!(matches!(
            request(&mut host, Request::Reset),
            Err(StructuredError {
                code: ErrorCode::Unsupported,
                ..
            })
        ));
        assert!(matches!(
            request(
                &mut host,
                Request::Modify {
                    target: Target::Root,
                    path: "x".to_owned(),
                    value: Value::Null,
                }
            ),
            Err(StructuredError {
                code: ErrorCode::Unsupported,
                ..
            })
        ));
    }

    #[test]
    fn unsupported_input_forms_are_rejected_before_submission() {
        let unsupported = [
            VirtualInput::Pointer {
                space: PointerSpace::Window,
                x: 1.0,
                y: 1.0,
            },
            VirtualInput::Button {
                button: MouseButton::Right,
                state: ButtonState::Down,
            },
            VirtualInput::Key {
                key: "A".to_owned(),
                state: ButtonState::Down,
            },
            VirtualInput::Focus { focused: true },
            VirtualInput::Leave,
            VirtualInput::Resize {
                width_physical: 1,
                height_physical: 1,
                scale: 1.0,
            },
        ];
        for event in unsupported {
            assert!(matches!(
                validate_input_form(&event),
                Err(StructuredError {
                    code: ErrorCode::Unsupported,
                    ..
                })
            ));
        }
    }

    #[test]
    fn request_boundary_rejects_invalid_forms_with_a_live_synthetic_player() {
        let mut host = NativeBevyHost::new();
        let witness = host.install_test_player(initialized_probe_player());
        let object = Target::Object(ObjectHandle {
            session_id: 1,
            generation: 1,
            object_id: 1,
        });
        assert!(matches!(
            request(
                &mut host,
                Request::Inspect {
                    target: object,
                    path: "current_frame".to_owned(),
                }
            ),
            Err(StructuredError {
                code: ErrorCode::InvalidHandle,
                ..
            })
        ));
        assert!(matches!(
            request(
                &mut host,
                Request::Invoke {
                    target: Target::Object(ObjectHandle {
                        session_id: 1,
                        generation: 1,
                        object_id: 1,
                    }),
                    function: "go".to_owned(),
                    arguments: vec![],
                }
            ),
            Err(StructuredError {
                code: ErrorCode::InvalidHandle,
                ..
            })
        ));
        assert!(matches!(
            request(
                &mut host,
                Request::Inspect {
                    target: Target::Root,
                    path: "unsupported_path".to_owned(),
                }
            ),
            Err(StructuredError {
                code: ErrorCode::Unsupported,
                ..
            })
        ));
        assert!(matches!(
            request(
                &mut host,
                Request::Inspect {
                    target: Target::Root,
                    path: "globals.bad-name".to_owned(),
                }
            ),
            Err(StructuredError {
                code: ErrorCode::InvalidRequest,
                ..
            })
        ));
        assert!(matches!(
            request(
                &mut host,
                Request::Invoke {
                    target: Target::Root,
                    function: "bad-name".to_owned(),
                    arguments: vec![],
                }
            ),
            Err(StructuredError {
                code: ErrorCode::InvalidRequest,
                ..
            })
        ));
        assert!(matches!(
            request(
                &mut host,
                Request::Invoke {
                    target: Target::Root,
                    function: "go".to_owned(),
                    arguments: vec![json!([1, 2])],
                }
            ),
            Err(StructuredError {
                code: ErrorCode::InvalidRequest,
                ..
            })
        ));
        assert!(host.is_started());
        assert!(matches!(
            host.submit(HostOperation::Shutdown),
            Ok(Response::Acknowledged)
        ));
        assert_retired(witness);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn repeated_start_is_rejected_without_replacing_the_existing_player() {
        let mut host = NativeBevyHost::new();
        let witness = host.install_test_player(initialized_probe_player());
        let result = request(
            &mut host,
            Request::Start {
                config: json!({"invalid": true}),
            },
        );
        assert!(matches!(
            result,
            Err(StructuredError {
                code: ErrorCode::InvalidRequest,
                ..
            })
        ));
        assert!(host.is_started());
        assert!(matches!(
            host.submit(HostOperation::Shutdown),
            Ok(Response::Acknowledged)
        ));
        assert_retired(witness);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn time_boundaries_preserve_elapsed_time_and_zero_uses_executor() {
        let mut host = NativeBevyHost::new();
        let witness = host.install_test_player(initialized_probe_player());
        assert!(matches!(
            request(&mut host, Request::Advance { duration_us: 0 }),
            Ok(Response::Acknowledged)
        ));
        assert_eq!(host.test_elapsed_us(), 0);
        assert!(!host.test_has_pending_operation());
        assert!(host.validate_advance(MAX_ADVANCE_US).is_ok());
        assert!(matches!(
            request(
                &mut host,
                Request::Advance {
                    duration_us: MAX_ADVANCE_US + 1,
                }
            ),
            Err(StructuredError {
                code: ErrorCode::InvalidRequest,
                ..
            })
        ));
        assert_eq!(host.test_elapsed_us(), 0);
        assert!(!host.test_has_pending_operation());
        host.test_set_elapsed_us(u64::MAX);
        assert!(matches!(
            request(&mut host, Request::Advance { duration_us: 1 }),
            Err(StructuredError {
                code: ErrorCode::InvalidRequest,
                ..
            })
        ));
        assert_eq!(host.test_elapsed_us(), u64::MAX);
        assert!(!host.test_has_pending_operation());
        assert!(matches!(
            host.submit(HostOperation::Shutdown),
            Ok(Response::Acknowledged)
        ));
        assert_retired(witness);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn runtime_error_leaves_time_unchanged_and_host_usable() {
        let mut host = NativeBevyHost::new();
        let witness = host.install_test_player(initialized_probe_player());
        let error = host
            .submit(HostOperation::Invoke {
                function: "native_missing_handler".to_owned(),
                arguments: vec![],
            })
            .expect_err("unknown handler must be a runtime error");
        assert!(matches!(error.code, ErrorCode::Runtime));
        assert_eq!(host.test_elapsed_us(), 0);
        assert!(matches!(
            host.submit(HostOperation::Advance(0)),
            Ok(Response::Acknowledged)
        ));
        assert_eq!(host.test_elapsed_us(), 0);
        assert!(host.is_started());
        assert!(matches!(
            host.submit(HostOperation::Shutdown),
            Ok(Response::Acknowledged)
        ));
        assert_retired(witness);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    #[ignore = "requires SPYBOT_RESOURCE_ROOT and the native rendering backend"]
    fn spybot_dual_host_interleaves_full_dcr_lifecycles() {
        let Some(first_config) = spybot_start_config_from_env() else {
            eprintln!("SPYBOT_RESOURCE_ROOT is unset; skipping dual-host proof");
            return;
        };
        let second_config = spybot_start_config_from_env()
            .expect("SPYBOT_RESOURCE_ROOT must remain available for both hosts");
        let admission = TestPlayerAdmission::acquire();
        let mut first = NativeBevyHost::new_with_test_admission(admission.clone());
        let mut second = NativeBevyHost::new_with_test_admission(admission.clone());
        let first_callbacks = first.install_callback_observer();
        let second_callbacks = second.install_callback_observer();

        assert!(matches!(
            first.submit(HostOperation::Start(first_config)),
            Ok(Response::Started { .. })
        ));
        assert!(matches!(
            second.submit(HostOperation::Start(second_config)),
            Ok(Response::Started { .. })
        ));
        let first_witness = first
            .test_live_teardown_witness()
            .expect("first host must expose a teardown witness");
        let second_witness = second
            .test_live_teardown_witness()
            .expect("second host must expose a teardown witness");
        let first_owner_key = first_witness.owner.key();
        let second_owner_key = second_witness.owner.key();
        assert_ne!(first_owner_key, second_owner_key);
        assert_eq!(first_witness.player_id, second_witness.player_id);

        let second_initial_frame = single_observation(&mut second, "current_frame");
        let second_initial_input = single_observation(&mut second, "input_state");
        assert!(matches!(
            first.submit(HostOperation::Advance(50_000)),
            Ok(Response::Acknowledged)
        ));
        assert_eq!(first.test_elapsed_us(), 50_000);
        assert_eq!(second.test_elapsed_us(), 0);
        assert_eq!(
            single_observation(&mut second, "current_frame"),
            second_initial_frame
        );
        assert_eq!(first_callbacks.borrow().len(), 0);
        assert_eq!(second_callbacks.borrow().len(), 0);
        assert!(matches!(
            first.submit(HostOperation::Input(VirtualInput::Pointer {
                space: PointerSpace::Stage,
                x: 200.0,
                y: 350.0,
            })),
            Ok(Response::Acknowledged)
        ));
        assert_eq!(
            single_observation(&mut second, "input_state"),
            second_initial_input
        );
        assert!(matches!(
            second.submit(HostOperation::Input(VirtualInput::Pointer {
                space: PointerSpace::Stage,
                x: 200.0,
                y: 350.0,
            })),
            Ok(Response::Acknowledged)
        ));
        assert!(matches!(
            second.submit(HostOperation::Advance(50_000)),
            Ok(Response::Acknowledged)
        ));
        for _ in 0..47 {
            let (first_result, second_result) = pair_submit(&mut first, &mut second, || {
                HostOperation::Advance(50_000)
            });
            assert!(matches!(first_result, Response::Acknowledged));
            assert!(matches!(second_result, Response::Acknowledged));
        }
        assert_eq!(first.test_elapsed_us(), 2_400_000);
        assert_eq!(second.test_elapsed_us(), 2_400_000);

        let (first_frame, second_frame) = pair_observation(&mut first, &mut second, "current_frame");
        assert_eq!(first_frame, json!(5));
        assert_eq!(first_frame, second_frame);
        let (first_label, second_label) = pair_observation(&mut first, &mut second, "current_label");
        assert_eq!(first_label, json!("title"));
        assert_eq!(first_label, second_label);
        let (first_flash, second_flash) = pair_observation(&mut first, &mut second, "flash_instances");
        assert_eq!(first_flash, second_flash);
        assert_eq!(first_flash["instances"][0]["sprite"], json!(1));
        assert_eq!(first_flash["instances"][0]["generation"], json!(1));
        assert_eq!(first_flash["instances"][0]["current_frame"], json!(419));
        let (first_init, second_init) = pair_observation(&mut first, &mut second, "init_state");
        assert_eq!(first_init, second_init);
        assert_eq!(first_init["pending_flash_actions"], json!([]));
        assert_eq!(first_init["flash_bindings"][0]["asserted_frame"], json!(371));

        for callbacks in [&first_callbacks, &second_callbacks] {
            let callbacks = callbacks.borrow();
            assert_eq!(callbacks.len(), 1);
            assert_eq!(callbacks[0].now_us, 2_400_000);
        }
        assert_eq!(first_callbacks.borrow()[0].owner, first_owner_key);
        assert_eq!(second_callbacks.borrow()[0].owner, second_owner_key);

        let mut rgba = Vec::new();
        let mut pcm = Vec::new();
        let mut source = Vec::new();
        let record_rgba = |name: &str,
                           first: &mut NativeBevyHost,
                           second: &mut NativeBevyHost,
                           rgba: &mut Vec<Value>,
                           source: &mut Vec<Value>| {
            let (first_capture, _) = pair_capture(first, second, CaptureKind::Rgba);
            rgba.push(json!({"name": name, "capture": capture_metadata(&first_capture)}));
            let (first_frame, second_frame) = pair_observation(first, second, "current_frame");
            let (first_label, second_label) = pair_observation(first, second, "current_label");
            let (first_input, second_input) = pair_observation(first, second, "input_state");
            assert_eq!(first_frame, second_frame);
            assert_eq!(first_label, second_label);
            assert_equal_input_state(
                &first_input,
                &second_input,
                first_owner_key,
                second_owner_key,
            );
            source.push(json!({
                "name": name,
                "frame": first_frame,
                "label": first_label,
                "input": first_input,
            }));
        };
        let record_pcm = |name: &str,
                          first: &mut NativeBevyHost,
                          second: &mut NativeBevyHost,
                          pcm: &mut Vec<Value>| {
            let (first_capture, _) = pair_capture(first, second, CaptureKind::Pcm);
            pcm.push(json!({"name": name, "capture": capture_metadata(&first_capture)}));
        };

        let (first_capture, _) = pair_capture(&mut first, &mut second, CaptureKind::Rgba);
        rgba.push(json!({"name": "t2400", "capture": capture_metadata(&first_capture)}));
        let (first_pcm, _) = pair_capture(&mut first, &mut second, CaptureKind::Pcm);
        pcm.push(json!({"name": "t2400", "capture": capture_metadata(&first_pcm)}));

        let (first_result, second_result) = pair_submit(&mut first, &mut second, || {
            HostOperation::Advance(50_000)
        });
        assert!(matches!(first_result, Response::Acknowledged));
        assert!(matches!(second_result, Response::Acknowledged));
        record_rgba("settled_t2450", &mut first, &mut second, &mut rgba, &mut source);
        record_pcm("settled_t2450", &mut first, &mut second, &mut pcm);

        for (name, event) in [
            (
                "hover_start",
                VirtualInput::Pointer {
                    space: PointerSpace::Stage,
                    x: 322.0,
                    y: 381.0,
                },
            ),
            (
                "press_start",
                VirtualInput::Button {
                    button: MouseButton::Left,
                    state: ButtonState::Down,
                },
            ),
            (
                "move_outside",
                VirtualInput::Pointer {
                    space: PointerSpace::Stage,
                    x: 200.0,
                    y: 350.0,
                },
            ),
            (
                "release_outside",
                VirtualInput::Button {
                    button: MouseButton::Left,
                    state: ButtonState::Up,
                },
            ),
        ] {
            let first_result = first.submit(HostOperation::Input(event));
            let second_event = match name {
                "hover_start" | "move_outside" => VirtualInput::Pointer {
                    space: PointerSpace::Stage,
                    x: if name == "hover_start" { 322.0 } else { 200.0 },
                    y: if name == "hover_start" { 381.0 } else { 350.0 },
                },
                "press_start" => VirtualInput::Button {
                    button: MouseButton::Left,
                    state: ButtonState::Down,
                },
                "release_outside" => VirtualInput::Button {
                    button: MouseButton::Left,
                    state: ButtonState::Up,
                },
                _ => unreachable!(),
            };
            let second_result = second.submit(HostOperation::Input(second_event));
            assert!(matches!(first_result, Ok(Response::Acknowledged)));
            assert!(matches!(second_result, Ok(Response::Acknowledged)));
            record_rgba(name, &mut first, &mut second, &mut rgba, &mut source);
        }

        let (first_result, second_result) = pair_submit(&mut first, &mut second, || {
            HostOperation::Advance(1_000_000)
        });
        assert!(matches!(first_result, Response::Acknowledged));
        assert!(matches!(second_result, Response::Acknowledged));
        record_rgba(
            "cancel_tail_t3450",
            &mut first,
            &mut second,
            &mut rgba,
            &mut source,
        );
        record_pcm("cancel_tail_t3450", &mut first, &mut second, &mut pcm);

        for (name, event) in [
            (
                "reenter_start",
                VirtualInput::Pointer {
                    space: PointerSpace::Stage,
                    x: 322.0,
                    y: 381.0,
                },
            ),
            (
                "press_start_again",
                VirtualInput::Button {
                    button: MouseButton::Left,
                    state: ButtonState::Down,
                },
            ),
            (
                "drag_outside_again",
                VirtualInput::Pointer {
                    space: PointerSpace::Stage,
                    x: 200.0,
                    y: 350.0,
                },
            ),
            (
                "reenter_start_again",
                VirtualInput::Pointer {
                    space: PointerSpace::Stage,
                    x: 322.0,
                    y: 381.0,
                },
            ),
            (
                "release_inside",
                VirtualInput::Button {
                    button: MouseButton::Left,
                    state: ButtonState::Up,
                },
            ),
        ] {
            let first_result = first.submit(HostOperation::Input(event));
            let second_event = match name {
                "reenter_start" | "reenter_start_again" => VirtualInput::Pointer {
                    space: PointerSpace::Stage,
                    x: 322.0,
                    y: 381.0,
                },
                "press_start_again" => VirtualInput::Button {
                    button: MouseButton::Left,
                    state: ButtonState::Down,
                },
                "drag_outside_again" => VirtualInput::Pointer {
                    space: PointerSpace::Stage,
                    x: 200.0,
                    y: 350.0,
                },
                "release_inside" => VirtualInput::Button {
                    button: MouseButton::Left,
                    state: ButtonState::Up,
                },
                _ => unreachable!(),
            };
            let second_result = second.submit(HostOperation::Input(second_event));
            assert!(matches!(first_result, Ok(Response::Acknowledged)));
            assert!(matches!(second_result, Ok(Response::Acknowledged)));
            record_rgba(name, &mut first, &mut second, &mut rgba, &mut source);
        }

        let (first_result, second_result) = pair_submit(&mut first, &mut second, || {
            HostOperation::Advance(50_000)
        });
        assert!(matches!(first_result, Response::Acknowledged));
        assert!(matches!(second_result, Response::Acknowledged));
        record_rgba(
            "activated_t3500",
            &mut first,
            &mut second,
            &mut rgba,
            &mut source,
        );
        let (first_frame, _) = pair_observation(&mut first, &mut second, "current_frame");
        assert_eq!(first_frame, json!(8));
        let (first_label, _) = pair_observation(&mut first, &mut second, "current_label");
        assert_eq!(first_label, json!("menu"));

        let (first_result, second_result) = pair_submit(&mut first, &mut second, || {
            HostOperation::Advance(950_000)
        });
        assert!(matches!(first_result, Response::Acknowledged));
        assert!(matches!(second_result, Response::Acknowledged));
        record_rgba(
            "activation_tail_t4450",
            &mut first,
            &mut second,
            &mut rgba,
            &mut source,
        );
        record_pcm("activation_tail_t4450", &mut first, &mut second, &mut pcm);

        let first_flash_handle = first
            .test_live_native_flash()
            .expect("first host must expose native Flash before retirement");
        let stale_session = first_witness.session.clone();
        let stale_player_id = first_witness.player_id;
        let stale_owner = first_witness.owner.clone();
        let mut stale_frame_pump = crate::player::session::NativeFramePump::new(
            first_witness.session.clone(),
            first_witness.player_id,
            first_witness.owner.clone(),
        );
        let stale_input_pump = crate::player::session::NativeInputPump::new(
            first_witness.session.clone(),
            first_witness.player_id,
            first_witness.owner.clone(),
        );
        let first_callback_count = first_callbacks.borrow().len();
        let stale_callback = {
            let observed = first_callbacks.borrow()[0].clone();
            crate::native_flash::NativeFlashCallback {
                sprite: observed.sprite,
                generation: observed.generation,
                cast_lib: observed.cast_lib,
                cast_member: observed.cast_member,
                url: observed.url,
            }
        };
        let stale_callback_delivery = Rc::new(RefCell::new(Vec::new()));
        assert!(matches!(
            first.submit(HostOperation::Shutdown),
            Ok(Response::Acknowledged)
        ));
        assert_retired(first_witness);
        assert_live(&second_witness);
        assert!(async_std::task::block_on(stale_frame_pump.advance_to_us(4_500_000)).is_err());
        assert!(async_std::task::block_on(stale_input_pump.mouse_move(200, 350)).is_err());
        assert!(first_flash_handle
            .borrow_mut()
            .dispatch_mouse(
                &stale_session,
                stale_player_id,
                &stale_owner,
                1,
                1,
                "move",
                0,
                0,
                1,
                1,
            )
            .is_err());
        assert!(!async_std::task::block_on(
            stale_frame_pump.test_dispatch_native_flash_callback_if_current(
                stale_callback,
                stale_callback_delivery.clone(),
            )
        )
        .expect("stale callback dispatch must complete"));
        assert!(stale_callback_delivery.borrow().is_empty());
        assert_eq!(first_callbacks.borrow().len(), first_callback_count);
        assert!(second_callbacks
            .borrow()
            .iter()
            .all(|callback| callback.owner == second_witness.owner.key()));

        assert!(matches!(
            second.submit(HostOperation::Advance(50_000)),
            Ok(Response::Acknowledged)
        ));
        assert_eq!(second.test_elapsed_us(), 4_500_000);
        let second_capture = single_capture(&mut second, CaptureKind::Rgba);
        assert_eq!(second_capture["value"]["kind"], json!("rgba"));
        let second_frame = single_observation(&mut second, "current_frame");
        assert_eq!(second_frame, json!(8));
        assert!(matches!(
            second.submit(HostOperation::Shutdown),
            Ok(Response::Acknowledged)
        ));
        assert_retired(second_witness);

        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "dcr_sha256": "ddf24b667a8d014856d9db42e1658cbf9714847f1cadc2c2c14d21f8e5950c77",
                "owner_keys": {
                    "first": format!("{:?}", first_owner_key),
                    "second": format!("{:?}", second_owner_key),
                },
                "rgba": rgba,
                "pcm": pcm,
                "source": source,
                "stale_frame_rejected": true,
                "stale_input_rejected": true,
                "stale_flash_input_rejected": true,
                "stale_callback_rejected": true,
                "first_retired": true,
                "second_retired": true,
            }))
            .expect("dual-host evidence must serialize")
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    #[ignore = "requires SPYBOT_RESOURCE_ROOT and the native rendering backend"]
    fn spybot_production_host_observes_callback_and_retires_owner() {
        let Some(config) = spybot_start_config_from_env() else {
            eprintln!("SPYBOT_RESOURCE_ROOT is unset; skipping full-DCR host proof");
            return;
        };
        let mut host = NativeBevyHost::new();
        let callbacks = host.install_callback_observer();
        assert!(matches!(
            host.submit(HostOperation::Start(config)),
            Ok(Response::Started { .. })
        ));
        for _ in 0..48 {
            assert!(matches!(
                host.submit(HostOperation::Advance(50_000)),
                Ok(Response::Acknowledged)
            ));
        }
        assert_eq!(host.test_elapsed_us(), 2_400_000);
        let inspect = |host: &mut NativeBevyHost, path: &str| match host
            .submit(HostOperation::Inspect(path.to_owned()))
            .expect("production host inspection must succeed")
        {
            Response::Observation(value) => value,
            response => panic!("expected observation for {path}, got {response:?}"),
        };
        assert_eq!(inspect(&mut host, "current_frame"), json!(5));
        assert_eq!(inspect(&mut host, "current_label"), json!("title"));
        let flash = inspect(&mut host, "flash_instances");
        assert_eq!(flash["instances"].as_array().map(Vec::len), Some(1));
        assert_eq!(flash["instances"][0]["sprite"], json!(1));
        assert_eq!(flash["instances"][0]["generation"], json!(1));
        assert_eq!(flash["instances"][0]["current_frame"], json!(419));
        let init = inspect(&mut host, "init_state");
        assert_eq!(init["pending_flash_actions"], json!([]));
        assert_eq!(init["flash_bindings"][0]["asserted_frame"], json!(371));
        assert_eq!(init["flash_bindings"][0]["cast_lib"], json!(2));
        assert_eq!(init["flash_bindings"][0]["cast_member"], json!(3));
        let witness = host
            .test_live_teardown_witness()
            .expect("started host must expose a teardown witness");
        let callback_values = callbacks.borrow();
        assert_eq!(callback_values.len(), 1);
        let callback = &callback_values[0];
        assert_eq!(callback.now_us, 2_400_000);
        assert_eq!(callback.owner, witness.owner.key());
        assert_eq!(callback.sprite, 1);
        assert_eq!(callback.generation, 1);
        assert_eq!(callback.cast_lib, 2);
        assert_eq!(callback.cast_member, 3);
        assert_eq!(callback.url, "lingo:introTitleReady()");
        drop(callback_values);
        assert!(matches!(
            host.submit(HostOperation::Shutdown),
            Ok(Response::Acknowledged)
        ));
        assert_retired(witness);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn failed_start_retains_no_player_and_retires_all_local_owner_bindings() {
        let mut host = NativeBevyHost::new();
        let movie = std::fs::read(PROBE_MOVIE).expect("probe movie must be readable");
        let result = host.submit(HostOperation::Start(StartConfig {
            adapter: AdapterConfig {
                backend: "dirplayer-native".to_owned(),
                resource_root: Path::new(PROBE_MOVIE)
                    .parent()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned(),
                movie: Path::new(PROBE_MOVIE)
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned(),
                source_dcr_sha256: hex_sha256(&movie),
                loading_policy: "single-dcr".to_owned(),
                resource_aliases: None,
                presentation_viewport: Some(PresentationViewportConfig {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                }),
            },
            seed: 0,
            clock: ClockConfig {
                epoch_us: 0,
                frame_rate_num: 61,
                frame_rate_den: 1,
            },
            loading: LoadingConfig {
                fixture: "probe".to_owned(),
            },
        }));
        assert!(
            matches!(
                result,
                Err(StructuredError {
                    code: ErrorCode::InvalidRequest,
                    ..
                })
            ),
            "expected tempo validation failure, got {result:?}"
        );
        assert!(!host.is_started());
        assert_eq!(host.test_elapsed_us(), 0);
        assert!(host.test_pointer_is_none());
        assert_retired(host.take_start_failure_witness().expect("start witness"));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn transport_eof_read_and_write_failures_retire_the_host() {
        let cases: [(&str, Box<dyn Read>, Box<dyn Write>); 3] = [
            (
                "eof",
                Box::new(Cursor::new(Vec::<u8>::new())),
                Box::new(Vec::<u8>::new()),
            ),
            ("read", Box::new(ReadFailure), Box::new(Vec::<u8>::new())),
            (
                "write",
                Box::new(Cursor::new(
                    b"{\"version\":1,\"request_id\":1,\"op\":\"discover\"}\n".to_vec(),
                )),
                Box::new(WriteFailure),
            ),
        ];
        for (name, mut input, mut output) in cases {
            let mut host = NativeBevyHost::new();
            let witness = host.install_test_player(
                TestPlayer::try_new()
                    .unwrap_or_else(|_| panic!("{name}: test player construction failed")),
            );
            let result = run_with_io(&mut input, &mut output, &mut host);
            if name == "eof" {
                assert!(result.is_ok());
            } else {
                assert!(result.is_err());
            }
            assert!(host.is_shutdown());
            assert_retired(witness);
        }
    }

    #[test]
    fn rejects_duplicate_targets() {
        let mut targets = HashSet::new();
        let target = PathBuf::from("/resource-root/shared.cct");
        assert!(insert_unique_target(&mut targets, target.clone(), "a.cct").is_ok());
        assert!(insert_unique_target(&mut targets, target, "b.cct").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!(
            "dirplayer-alias-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        let outside = root.with_extension("outside");
        std::fs::write(&outside, b"outside").unwrap();
        symlink(&outside, root.join("escape.cct")).unwrap();
        assert!(resolve_alias_path(&root, "escape.cct").is_err());
        std::fs::remove_file(root.join("escape.cct")).unwrap();
        std::fs::remove_file(outside).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
