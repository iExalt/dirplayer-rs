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

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use url::Url;

use crate::director::file::{DirectorFile, read_director_file_bytes};
use crate::player::testing::{NativeGlobalReadError, NativeGlobalValue, TestPlayer};

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
enum Request {
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
enum Response {
    Capabilities(Capabilities),
    Started { session: SessionHandle },
    Observation(Value),
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
enum CaptureKind {
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
enum VirtualInput {
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
struct SessionHandle {
    id: u64,
    generation: u64,
}

#[derive(Debug, Serialize)]
struct StructuredError {
    code: ErrorCode,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum ErrorCode {
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
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
enum Capture {
    Rgba(RgbaCapture),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StartConfig {
    adapter: AdapterConfig,
    seed: u64,
    clock: ClockConfig,
    loading: LoadingConfig,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdapterConfig {
    backend: String,
    resource_root: String,
    movie: String,
    source_dcr_sha256: String,
    loading_policy: String,
    resource_aliases: Option<BTreeMap<String, ExternalCastAlias>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExternalCastAlias {
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

    #[cfg(test)]
    pub(crate) fn from_test_entries(
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
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClockConfig {
    epoch_us: i64,
    frame_rate_num: u32,
    frame_rate_den: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LoadingConfig {
    fixture: String,
}

struct Worker {
    player: Option<TestPlayer>,
    elapsed_us: u64,
    pointer: Option<(i32, i32)>,
    session: SessionHandle,
    runtime_version: String,
    shutdown: bool,
}

impl Worker {
    fn new() -> Self {
        Self {
            player: None,
            elapsed_us: 0,
            pointer: None,
            session: SessionHandle {
                id: 1,
                generation: 1,
            },
            runtime_version: env!("CARGO_PKG_VERSION").to_owned(),
            shutdown: false,
        }
    }

    fn request(&mut self, request: Request) -> Result<Response, StructuredError> {
        match request {
            Request::Discover => Ok(Response::Capabilities(Capabilities {
                runtime: "dirplayer-native".to_owned(),
                runtime_version: self.runtime_version.clone(),
                operations: vec![
                    Capability::StateInspection,
                    Capability::VirtualInput,
                    Capability::ControlledTime,
                    Capability::RgbaCapture,
                ],
            })),
            Request::Start { config } => self.start(config),
            Request::Reset | Request::Modify { .. } | Request::Invoke { .. } => Err(unsupported(
                "native worker does not implement reset, mutation, or invocation",
            )),
            Request::Inspect { target, path } => {
                self.require_started()?;
                if !matches!(target, Target::Root) {
                    return Err(error(
                        ErrorCode::InvalidHandle,
                        "native inspection accepts only the root target",
                    ));
                }
                self.inspect(&path)
            }
            Request::Input { event } => {
                self.require_started()?;
                self.input(event)
            }
            Request::Advance { duration_us } => {
                self.require_started()?;
                self.advance(duration_us)
            }
            Request::Capture {
                kind: CaptureKind::Pcm,
            } => Err(unsupported(
                "native Director worker does not implement PCM capture",
            )),
            Request::Capture {
                kind: CaptureKind::Rgba,
            } => {
                self.require_started()?;
                self.capture()
            }
            Request::Shutdown => {
                self.player.take();
                self.shutdown = true;
                Ok(Response::Acknowledged)
            }
        }
    }

    fn start(&mut self, raw: Value) -> Result<Response, StructuredError> {
        if self.player.is_some() {
            return Err(error(
                ErrorCode::InvalidRequest,
                "one native worker process owns one scenario",
            ));
        }
        validate_parent_storage()?;
        let config: StartConfig = serde_json::from_value(raw).map_err(|error| {
            error_with(
                ErrorCode::InvalidRequest,
                format!("invalid deterministic start configuration: {error}"),
                None,
            )
        })?;
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

        let mut player = TestPlayer::try_new().map_err(runtime_error)?;
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
        let tempo = player.effective_tempo_quiet().map_err(runtime_error)?;
        if tempo != config.clock.frame_rate_num {
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

    fn require_started(&self) -> Result<(), StructuredError> {
        if self.player.is_some() {
            Ok(())
        } else {
            Err(error(
                ErrorCode::NotReady,
                "native worker has not started a session",
            ))
        }
    }

    fn inspect(&self, path: &str) -> Result<Response, StructuredError> {
        let Some(player) = self.player.as_ref() else {
            return Err(not_ready());
        };
        let value = match path {
            "current_frame" => json!(player.current_frame_quiet()),
            "simulation_time_us" => json!(self.elapsed_us),
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

    fn input(&mut self, event: VirtualInput) -> Result<Response, StructuredError> {
        match event {
            VirtualInput::Pointer {
                space: PointerSpace::Stage,
                x,
                y,
            } => {
                let x = checked_coordinate(x, "x")?;
                let y = checked_coordinate(y, "y")?;
                self.pointer = Some((x, y));
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
            VirtualInput::Button { .. } => Err(unsupported("only left button down is qualified")),
            VirtualInput::Key { .. } => Err(unsupported("keyboard input is not qualified")),
            VirtualInput::Focus { .. } | VirtualInput::Leave | VirtualInput::Resize { .. } => {
                Err(unsupported("this virtual input form is not qualified"))
            }
        }
    }

    fn advance(&mut self, duration_us: u64) -> Result<Response, StructuredError> {
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
        let Some(player) = self.player.as_mut() else {
            return Err(not_ready());
        };
        async_std::task::block_on(player.try_advance_to_us(target)).map_err(runtime_error)?;
        self.elapsed_us = target;
        Ok(Response::Acknowledged)
    }

    fn capture(&self) -> Result<Response, StructuredError> {
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
}

pub fn run() -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdin = stdin.lock();
    let stdout = io::stdout();
    let mut stdout = io::BufWriter::new(stdout.lock());
    let mut worker = Worker::new();
    loop {
        let Some(line) = read_bounded_line(&mut stdin)? else {
            break;
        };
        if line.len() > MAX_LINE_BYTES {
            write_response(
                &mut stdout,
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
                    &mut stdout,
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
            worker
                .request(envelope.request)
                .unwrap_or_else(Response::Error)
        };
        write_response(&mut stdout, envelope.request_id, response)?;
        if worker.shutdown {
            break;
        }
    }
    Ok(())
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
    match error {
        NativeGlobalReadError::Runtime(error) => runtime_error(error),
        NativeGlobalReadError::Unsupported { datum_type, reason } => error_with(
            ErrorCode::Unsupported,
            format!("global datum type {datum_type} is not representable in v1"),
            Some(json!({"datum_type": datum_type, "reason": reason})),
        ),
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
