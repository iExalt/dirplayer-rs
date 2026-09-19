use std::{path::{Path, PathBuf}, rc::Rc, time::Duration};
use std::collections::HashSet;
use std::sync::Mutex;

use async_std::channel;
use manual_future::ManualFuture;
use rand::SeedableRng;

use crate::director::file::read_director_file_bytes;
pub use crate::director::static_datum::StaticDatum;
use crate::player::{
    commands::{run_command_loop, PlayerVMCommand},
    session::{NativeAdvanceReport, NativeFramePump, NativeInputPump},
    PlayerVMExecutionItem,
};
pub use crate::player::testing_shared::{TestHarness, SnapshotOutput};
use crate::player::testing_shared::HarnessRuntime;

/// Global lock to ensure only one TestPlayer runs at a time.
/// The player uses global mutable statics, so tests must be serialized.
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// Native test harness. Wraps the global DirPlayer for in-memory testing.
pub struct TestPlayer {
    _tx: channel::Sender<PlayerVMExecutionItem>,
    _lock: std::sync::MutexGuard<'static, ()>,
    runtime: HarnessRuntime,
    native_presentation: Rc<crate::rendering::NativePresentationPolicy>,
    native_frame_pump: NativeFramePump,
    native_input_pump: NativeInputPump,
}

#[derive(Debug)]
pub(crate) enum NativeMovieLoadError {
    Unsupported(String),
    Runtime(crate::player::ScriptError),
}

/// Values exposed by the narrow native parity inspection bridge. This stays
/// separate from `StaticDatum`, whose legacy conversion intentionally maps
/// unsupported runtime values to Void for older tests.
#[derive(Debug, PartialEq)]
pub(crate) enum NativeGlobalValue {
    Int(i32),
    Float(f64),
    String(String),
    Symbol(String),
    Void,
    List(Vec<NativeGlobalValue>),
    PropList(Vec<(NativeGlobalValue, NativeGlobalValue)>),
    Point([f64; 2]),
    Rect([f64; 4]),
}

#[derive(Debug)]
pub(crate) enum NativeGlobalReadError {
    Unsupported {
        datum_type: &'static str,
        reason: Option<&'static str>,
    },
    Runtime(crate::player::ScriptError),
}

const MAX_NATIVE_GLOBAL_DEPTH: usize = 32;
const MAX_NATIVE_GLOBAL_NODES: usize = 4096;

impl From<crate::player::ScriptError> for NativeMovieLoadError {
    fn from(error: crate::player::ScriptError) -> Self {
        Self::Runtime(error)
    }
}

impl TestPlayer {
    pub fn new() -> Self {
        Self::try_new().expect("native test player construction failed")
    }

    pub(crate) fn try_new() -> Result<Self, crate::player::ScriptError> {
        let lock = match TEST_LOCK.lock() {
            Ok(lock) => lock,
            Err(poisoned) => poisoned.into_inner(),
        };

        let (tx, rx) = channel::unbounded();

        let runtime = HarnessRuntime::try_new(tx.clone())?;
        let native_presentation = Rc::new(crate::rendering::NativePresentationPolicy::new());
        runtime
            .session()
            .borrow_mut()
            .bind_native_presentation(runtime.player_id(), &native_presentation);
        let native_frame_pump = NativeFramePump::new(
            runtime.session(),
            runtime.player_id(),
            runtime.owner().clone(),
        );
        let native_input_pump = NativeInputPump::new(
            runtime.session(),
            runtime.player_id(),
            runtime.owner().clone(),
        );
        let command_session = runtime.session();
        let command_player_id = runtime.player_id();
        let command_owner = runtime.owner().clone();
        crate::player::spawn_player_local(async move {
            run_command_loop(rx, command_session, command_player_id, command_owner).await;
        });
        Ok(TestPlayer {
            _tx: tx,
            _lock: lock,
            runtime,
            native_presentation,
            native_frame_pump,
            native_input_pump,
        })
    }

    /// Initialize this native player at an explicit caller-owned simulation time.
    pub async fn init_movie_at(&mut self, now_ms: u64) {
        self.native_frame_pump
            .init_movie_at(now_ms)
            .await
            .unwrap_or_else(|error| panic!("native movie initialization failed: {}", error));
    }

    pub(crate) async fn try_init_movie_at_us(
        &mut self,
        now_us: u64,
    ) -> Result<(), crate::player::ScriptError> {
        self.native_frame_pump.init_movie_at_us(now_us).await
    }

    /// Advance this native player to an explicit, strictly later simulation time.
    pub async fn advance_frame_to(&mut self, now_ms: u64) -> bool {
        self.native_frame_pump
            .advance_frame_to(now_ms)
            .await
            .unwrap_or_else(|error| panic!("native frame advancement failed: {}", error))
            .0
    }

    /// Advance the native logical clock through all due timer/frame boundaries.
    pub async fn advance_to(&mut self, now_ms: u64) -> crate::player::session::NativeAdvanceReport {
        self.native_frame_pump
            .advance_to(now_ms)
            .await
            .unwrap_or_else(|error| panic!("native logical advancement failed: {}", error))
    }

    pub(crate) async fn try_advance_to_us(
        &mut self,
        now_us: u64,
    ) -> Result<crate::player::session::NativeAdvanceReport, crate::player::ScriptError> {
        self.native_frame_pump.advance_to_us(now_us).await
    }

    /// Fallible native logical advancement for boundary and owner tests.
    pub async fn try_advance_to(
        &mut self,
        now_ms: u64,
    ) -> Result<crate::player::session::NativeAdvanceReport, crate::player::ScriptError> {
        self.native_frame_pump.advance_to(now_ms).await
    }

    /// Consume native host-unsupported receipts; native logical timers should
    /// leave this sink empty while legacy/browser paths retain their receipts.
    pub fn take_native_timeout_host_unsupported(&mut self) -> Vec<crate::js_api::TimeoutHostDispatch> {
        self.runtime
            .session()
            .borrow_mut()
            .take_native_timeout_host_unsupported()
    }

    pub async fn native_mouse_down(&self, x: i32, y: i32) {
        self.native_input_pump
            .mouse_down(x, y)
            .await
            .unwrap_or_else(|error| panic!("native mouseDown failed: {}", error.message));
    }

    pub(crate) async fn try_native_mouse_down(
        &self,
        x: i32,
        y: i32,
    ) -> Result<(), crate::player::ScriptError> {
        self.native_input_pump.mouse_down(x, y).await
    }

    pub(crate) fn set_deterministic_seed(
        &self,
        seed: u32,
    ) -> Result<(), crate::player::ScriptError> {
        self.runtime
            .with_context(|context| {
                context.player.rng = rand::rngs::SmallRng::seed_from_u64(u64::from(seed));
                context.player.movie.random_seed = Some(seed as i32);
            })
            .ok_or_else(|| {
                crate::player::ScriptError::new("harness player was replaced".to_owned())
            })
    }

    pub(crate) async fn load_movie_quiet(
        &mut self,
        path: &str,
        data_bytes: Vec<u8>,
    ) -> Result<(), NativeMovieLoadError> {
        let movie_path = Path::new(path);
        let file_name = movie_path
            .file_name()
            .ok_or_else(|| {
                NativeMovieLoadError::Runtime(crate::player::ScriptError::new(
                    "movie path has no file name".to_owned(),
                ))
            })?
            .to_string_lossy()
            .to_string();
        let dir = movie_path.parent().ok_or_else(|| {
            NativeMovieLoadError::Runtime(crate::player::ScriptError::new(
                "movie path has no parent".to_owned(),
            ))
        })?;
        let base_url = url::Url::from_directory_path(dir)
            .map_err(|_| {
                NativeMovieLoadError::Runtime(crate::player::ScriptError::new(
                    "movie parent is not a file URL path".to_owned(),
                ))
            })?
            .to_string();
        let dir_file = read_director_file_bytes(&data_bytes, &file_name, &base_url).map_err(
            |error| NativeMovieLoadError::Runtime(crate::player::ScriptError::new(error)),
        )?;
        let external_casts: Vec<_> = dir_file
            .cast_entries
            .iter()
            .filter(|entry| !entry.file_path.is_empty())
            .map(|entry| entry.file_path.clone())
            .collect();
        let embedded_flash = dir_file
            .casts
            .iter()
            .flat_map(|cast| cast.members.values())
            .filter(|member| {
                matches!(
                    member.chunk.specific_data,
                    crate::director::chunks::cast_member::CastMemberSpecificData::Flash(_)
                )
            })
            .count();
        let javascript_scripts = dir_file
            .casts
            .iter()
            .flat_map(|cast| cast.lctx.iter())
            .flat_map(|context| context.scripts.values())
            .filter(|script| {
                script.literals.iter().any(|literal| {
                    matches!(literal, crate::director::lingo::datum::Datum::JavaScript(_))
                })
            })
            .count();
        validate_native_movie_features(&external_casts, embedded_flash, javascript_scripts)?;
        self.runtime
            .with_context(|context| {
                context.player.is_playing = true;
                context.player.is_script_paused = false;
            })
            .ok_or_else(|| {
                NativeMovieLoadError::Runtime(crate::player::ScriptError::new(
                    "harness player was replaced".to_owned(),
                ))
            })?;
        crate::player::load_movie_from_dir_owned(
            self.runtime.session(),
            self.runtime.player_id(),
            self.runtime.owner().clone(),
            dir_file,
        )
        .await
        .map_err(NativeMovieLoadError::Runtime)
    }

    pub(crate) async fn eval_datum_quiet(
        &self,
        command: &str,
    ) -> Result<StaticDatum, crate::player::ScriptError> {
        TestHarness::eval_datum(self, command).await
    }

    pub(crate) fn current_frame_quiet(&self) -> u32 {
        TestHarness::current_frame(self)
    }

    pub(crate) fn native_global_value_quiet(
        &self,
        name: &str,
    ) -> Result<Option<NativeGlobalValue>, NativeGlobalReadError> {
        self.runtime
            .with_context(|context| {
                let lower_name = name.to_ascii_lowercase();
                let Some(value_ref) = context
                    .player
                    .globals
                    .iter()
                    .find(|(symbol, _)| {
                        context.symbols.lower(symbol).is_ok_and(|name| name == lower_name)
                    })
                    .map(|(_, value)| value)
                else {
                    return Ok(None);
                };
                let mut active = HashSet::new();
                let mut nodes = 0;
                strict_native_global_value(
                    context.player,
                    context.symbols,
                    value_ref,
                    0,
                    &mut nodes,
                    &mut active,
                )
                .map(Some)
            })
            .ok_or_else(|| {
                NativeGlobalReadError::Runtime(crate::player::ScriptError::new(
                    "harness player was replaced".to_owned(),
                ))
            })?
    }

    pub(crate) fn effective_tempo_quiet(&self) -> Result<u32, crate::player::ScriptError> {
        self.runtime
            .with_context(|context| context.player.movie.get_effective_tempo())
            .ok_or_else(|| {
                crate::player::ScriptError::new("harness player was replaced".to_owned())
            })
    }

    pub(crate) fn snapshot_rgba_quiet(&self) -> Result<StageSnapshot, crate::player::ScriptError> {
        let bitmap = crate::rendering::snapshot_native_fresh_for_owner(
            &self.runtime.session(),
            self.runtime.player_id(),
            self.runtime.owner(),
        )?;
        Ok(StageSnapshot {
            width: bitmap.width as u32,
            height: bitmap.height as u32,
            data: bitmap.data,
        })
    }

}

fn strict_native_global_value(
    player: &crate::player::DirPlayer,
    symbols: &crate::player::symbols::symbol_table::SymbolTable,
    value_ref: &crate::player::DatumRef,
    depth: usize,
    nodes: &mut usize,
    active: &mut HashSet<usize>,
) -> Result<NativeGlobalValue, NativeGlobalReadError> {
    if depth > MAX_NATIVE_GLOBAL_DEPTH {
        return Err(NativeGlobalReadError::Unsupported {
            datum_type: "container",
            reason: Some("depth_limit"),
        });
    }
    if matches!(value_ref, crate::player::DatumRef::Void) {
        return Ok(NativeGlobalValue::Void);
    }
    *nodes += 1;
    if *nodes > MAX_NATIVE_GLOBAL_NODES {
        return Err(NativeGlobalReadError::Unsupported {
            datum_type: "container",
            reason: Some("node_limit"),
        });
    }
    let id = value_ref.unwrap();
    if !active.insert(id) {
        return Err(NativeGlobalReadError::Unsupported {
            datum_type: "container",
            reason: Some("cycle"),
        });
    }
    let datum = player
        .allocator
        .try_get_datum(value_ref)
        .ok_or_else(|| {
            NativeGlobalReadError::Runtime(crate::player::ScriptError::new(
                "global contains a foreign or stale datum reference".to_owned(),
            ))
        })?;
    let result = match datum {
        crate::director::lingo::datum::Datum::Int(value) => Ok(NativeGlobalValue::Int(*value)),
        crate::director::lingo::datum::Datum::Float(value) if value.is_finite() => {
            Ok(NativeGlobalValue::Float(*value))
        }
        crate::director::lingo::datum::Datum::Float(_) => Err(
            NativeGlobalReadError::Unsupported {
                datum_type: "float",
                reason: Some("non_finite"),
            },
        ),
        crate::director::lingo::datum::Datum::String(value) => {
            Ok(NativeGlobalValue::String(value.clone()))
        }
        crate::director::lingo::datum::Datum::Symbol(value) => symbols
            .display(value)
            .map(|display| NativeGlobalValue::Symbol(display.to_owned()))
            .map_err(|_| {
                NativeGlobalReadError::Runtime(crate::player::ScriptError::new(
                    "global contains a foreign symbol".to_owned(),
                ))
            }),
        crate::director::lingo::datum::Datum::Void => Ok(NativeGlobalValue::Void),
        crate::director::lingo::datum::Datum::List(_, values, _) => values
            .iter()
            .map(|value| {
                strict_native_global_value(player, symbols, value, depth + 1, nodes, active)
            })
            .collect::<Result<Vec<_>, _>>()
            .map(NativeGlobalValue::List),
        crate::director::lingo::datum::Datum::PropList(values, _) => values
            .iter()
            .map(|(key, value)| {
                Ok((
                    strict_native_global_value(player, symbols, key, depth + 1, nodes, active)?,
                    strict_native_global_value(player, symbols, value, depth + 1, nodes, active)?,
                ))
            })
            .collect::<Result<Vec<_>, NativeGlobalReadError>>()
            .map(NativeGlobalValue::PropList),
        crate::director::lingo::datum::Datum::Point(values, _)
            if values.iter().all(|value| value.is_finite()) =>
        {
            Ok(NativeGlobalValue::Point(*values))
        }
        crate::director::lingo::datum::Datum::Point(_, _) => {
            Err(NativeGlobalReadError::Unsupported {
                datum_type: "point",
                reason: Some("non_finite"),
            })
        }
        crate::director::lingo::datum::Datum::Rect(values, _)
            if values.iter().all(|value| value.is_finite()) =>
        {
            Ok(NativeGlobalValue::Rect(*values))
        }
        crate::director::lingo::datum::Datum::Rect(_, _) => {
            Err(NativeGlobalReadError::Unsupported {
                datum_type: "rect",
                reason: Some("non_finite"),
            })
        }
        _ => Err(NativeGlobalReadError::Unsupported {
            datum_type: datum.type_str(),
            reason: None,
        }),
    };
    active.remove(&id);
    result
}

fn validate_native_movie_features(
    external_casts: &[String],
    embedded_flash: usize,
    javascript_scripts: usize,
) -> Result<(), NativeMovieLoadError> {
    if !external_casts.is_empty() {
        return Err(NativeMovieLoadError::Unsupported(format!(
            "native single-dcr worker does not support external casts: {}",
            external_casts.join(", ")
        )));
    }
    if embedded_flash != 0 {
        return Err(NativeMovieLoadError::Unsupported(format!(
            "native Director worker does not support {embedded_flash} embedded Flash member(s)"
        )));
    }
    if javascript_scripts != 0 {
        return Err(NativeMovieLoadError::Unsupported(format!(
            "native Director worker does not support {javascript_scripts} JavaScript Lingo script(s)"
        )));
    }
    Ok(())
}

impl TestHarness for TestPlayer {
    fn harness_runtime(&self) -> &HarnessRuntime { &self.runtime }

    fn asset_path(&self, relative: &str) -> String {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let workspace_root = std::path::Path::new(manifest_dir).parent().unwrap();
        // Push each segment separately: `join()` on a "a/b/c" string keeps the
        // forward slashes verbatim, so on Windows this produced MIXED separators
        // (`E:\...\public\dcr_sulake/habbo_v7/habbo.dcr`). This is the path the
        // test hands to `load_movie`, which derives the movie's base URL from it.
        let mut abs = workspace_root.join("public");
        for segment in relative.split('/').filter(|s| !s.is_empty()) {
            abs.push(segment);
        }
        abs.to_string_lossy().to_string()
    }
    async fn load_movie(&mut self, path: &str) {
        crate::player::testing_shared::log_test_action(&format!("Load: {}", path));
        let abs_path = if Path::new(path).is_absolute() {
            path.to_string()
        } else {
            let manifest_dir = env!("CARGO_MANIFEST_DIR");
            let workspace_root = Path::new(manifest_dir).parent().unwrap();
            // Push each segment separately. `join()` on a "a/b/c" string keeps the
            // forward slashes verbatim, so on Windows the result came out with
            // MIXED separators (`E:\...\public\dcr_sulake/habbo_v7/habbo.dcr`).
            // `fs::read` tolerates that, but anything that parses or compares the
            // path (or derives a URL from it) should not have to.
            let mut abs = workspace_root.to_path_buf();
            for segment in path.split('/').filter(|s| !s.is_empty()) {
                abs.push(segment);
            }
            abs.to_string_lossy().to_string()
        };

        let data_bytes =
            std::fs::read(&abs_path).unwrap_or_else(|e| panic!("Failed to read {}: {}", abs_path, e));

        let file_name = Path::new(&abs_path)
            .file_name().unwrap().to_string_lossy().to_string();

        // A correct file URL for the movie's DIRECTORY, built with
        // `Url::from_directory_path` rather than string concatenation.
        //
        // `format!("file://{}", dir)` was wrong on every platform:
        //   * no trailing slash, so resolving a relative cast name against it
        //     REPLACES the last path segment instead of appending —
        //     `fuse_client.cct` landed beside the movie's folder, not inside it.
        //     That alone breaks macOS and Linux.
        //   * on Windows it additionally produced `file://E:\dir`, which parses
        //     with HOST = "e:" and an EMPTY path rather than a drive-letter path
        //     (a valid Windows file URL is `file:///E:/dir/`), so the resolved
        //     cast path was garbage and `std::fs::read` failed.
        // With the external casts failing to load, their movie scripts never
        // registered, so habbo's `startClient()` (which lives in fuse_client.cct)
        // raised "No built-in handler: startClient()" on the first frame.
        //
        // `Url::from_directory_path` is the portable answer: drive letters and
        // `\` on Windows, plain `/` paths on macOS/Linux, trailing slash on all
        // three.
        let dir = Path::new(&abs_path).parent().unwrap();
        let base_url = url::Url::from_directory_path(dir)
            .unwrap_or_else(|_| panic!("Could not build a file URL for {}", dir.display()))
            .to_string();

        let dir_file = read_director_file_bytes(&data_bytes, &file_name, &base_url)
            .unwrap_or_else(|e| panic!("Failed to parse {}: {:?}", file_name, e));

        self.runtime.with_context(|context| {
            context.player.is_playing = true;
            context.player.is_script_paused = false;
        });

        crate::player::load_movie_from_dir_owned(
            self.runtime.session(),
            self.runtime.player_id(),
            self.runtime.owner().clone(),
            dir_file,
        )
        .await
        .unwrap_or_else(|error| panic!("movie load failed: {}", error));
    }

    async fn step_frame(&mut self) -> bool {
        crate::player::fire_pending_timeouts_owned(
            self.runtime.session(),
            self.runtime.player_id(),
            self.runtime.owner().clone(),
        )
        .await
        .unwrap_or_else(|error| panic!("timeout dispatch failed: {}", error));
        let (is_playing, _) = crate::player::run_single_frame_owned(
            self.runtime.session(),
            self.runtime.player_id(),
            self.runtime.owner().clone(),
        )
        .await
        .unwrap_or_else(|error| panic!("frame execution failed: {}", error));
        let delay_ms = self.runtime.with_context(|context| {
            let tempo = context.player.movie.get_effective_tempo();
            if tempo > 0 { 1000 / tempo } else { 33 }
        }).unwrap_or(33);
        std::thread::sleep(std::time::Duration::from_millis(delay_ms as u64));
        is_playing
    }

    fn snapshot_stage(&self) -> SnapshotOutput {
        let bitmap = crate::rendering::snapshot_native_fresh_for_owner(
            &self.runtime.session(),
            self.runtime.player_id(),
            self.runtime.owner(),
        )
        .unwrap_or_else(|error| panic!("native stage snapshot failed: {}", error));
        SnapshotOutput::Rgba {
            width: bitmap.width as u32,
            height: bitmap.height as u32,
            data: bitmap.data,
        }
    }
}

impl Drop for TestPlayer {
    fn drop(&mut self) {
        self.runtime
            .session()
            .borrow_mut()
            .unbind_native_presentation(self.runtime.player_id());
        self.native_presentation.dispose();
        self.runtime.retire_current();
    }
}

/// Run an async test body using the async-std runtime (single-threaded).
/// Minimal `log` sink for native tests.
///
/// Without a logger installed, `log::error!` / `warn!` go nowhere — and since
/// `console_error!` / `console_warn!` map to those off the web build, every
/// script error a movie hits was silently discarded. The native e2e run would
/// then report only a downstream symptom ("Movie stopped while waiting for
/// member 'Logo'") with no hint of the actual Lingo failure that caused it.
struct NativeTestLogger;

impl log::Log for NativeTestLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Warn
    }
    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            eprintln!("[{}] {}", record.level(), record.args());
        }
    }
    fn flush(&self) {}
}

static NATIVE_TEST_LOGGER: NativeTestLogger = NativeTestLogger;

pub fn run_test<F: std::future::Future<Output = ()>>(f: F) {
    // `set_logger` errors if one is already installed (a second test in the same
    // process); that is fine, ignore it.
    let _ = log::set_logger(&NATIVE_TEST_LOGGER)
        .map(|()| log::set_max_level(log::LevelFilter::Warn));
    async_std::task::block_on(f);
}

// --- Snapshot comparison utilities ---

pub struct StageSnapshot {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl StageSnapshot {
    /// Create from a SnapshotOutput (native only).
    pub fn from_output(output: SnapshotOutput) -> Self {
        match output {
            SnapshotOutput::Rgba { width, height, data } => StageSnapshot { width, height, data },
            _ => panic!("Expected Rgba snapshot on native"),
        }
    }

    pub fn to_png(&self) -> Vec<u8> {
        use image::{ImageBuffer, RgbaImage};
        let img: RgbaImage = ImageBuffer::from_raw(self.width, self.height, self.data.clone())
            .expect("Failed to create image buffer");
        let mut buf: Vec<u8> = Vec::new();
        let encoder = image::codecs::png::PngEncoder::new(&mut buf);
        image::ImageEncoder::write_image(
            encoder, img.as_raw(), self.width, self.height,
            image::ExtendedColorType::Rgba8,
        ).expect("Failed to encode PNG");
        buf
    }

    /// Compare against a reference file.
    ///
    /// `snapshot_path` is `"suite/test"` (e.g. `"habbo/load"`).
    /// `name` is the snapshot step name (e.g. "preload").
    ///
    /// Files are stored as `snapshots/{suite}/native/{test}/{name}.png`.
    /// Returns `Ok(Some(ratio))` when a comparison was made and passed,
    /// `Ok(None)` when there is no reference or the reference was updated,
    /// and `Err` when the diff exceeds the threshold.
    pub fn assert_snapshot(&self, snapshot_path: &str, name: &str, max_diff_ratio: f64, pixel_tolerance: u8) -> Result<Option<f64>, String> {
        let (suite, test) = snapshot_path.split_once('/')
            .unwrap_or((snapshot_path, "default"));
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let base = Path::new(manifest_dir).join("tests/snapshots");
        let output_dir = base.join("output").join(suite).join("native").join(test);
        let reference_dir = base.join("reference").join(suite).join("native").join(test);

        std::fs::create_dir_all(&output_dir).unwrap();
        std::fs::create_dir_all(&reference_dir).unwrap();

        let file_name = format!("{}.png", name);
        let output_path = output_dir.join(&file_name);
        let reference_path = reference_dir.join(&file_name);

        let actual_png = self.to_png();
        std::fs::write(&output_path, &actual_png).unwrap();

        if std::env::var("SNAPSHOT_UPDATE").unwrap_or_default() == "1" {
            std::fs::write(&reference_path, &actual_png).unwrap();
            return Ok(None);
        }

        if reference_path.exists() {
            let reference_data = std::fs::read(&reference_path).unwrap();
            let reference_img = image::load_from_memory(&reference_data)
                .expect("Failed to decode reference PNG");
            let reference_rgba = reference_img.to_rgba8();

            let gw = reference_rgba.width();
            let gh = reference_rgba.height();
            if self.width != gw || self.height != gh {
                return Err(format!(
                    "Snapshot '{}' dimensions differ: actual {}x{} vs reference {}x{}",
                    name, self.width, self.height, gw, gh
                ));
            }

            let reference_raw = reference_rgba.into_raw();
            let pixel_count = (self.width * self.height) as usize;
            let mut diff_pixels = 0usize;
            let mut max_diff: u8 = 0;
            let mut diff_img = vec![0u8; pixel_count * 4];
            for i in 0..pixel_count {
                let off = i * 4;
                let dr = (self.data[off] as i16 - reference_raw[off] as i16).unsigned_abs() as u8;
                let dg = (self.data[off+1] as i16 - reference_raw[off+1] as i16).unsigned_abs() as u8;
                let db = (self.data[off+2] as i16 - reference_raw[off+2] as i16).unsigned_abs() as u8;
                let da = (self.data[off+3] as i16 - reference_raw[off+3] as i16).unsigned_abs() as u8;
                let ch_max = dr.max(dg).max(db).max(da);
                if ch_max > pixel_tolerance {
                    diff_pixels += 1;
                    max_diff = max_diff.max(ch_max);
                    // Red highlight for changed pixels
                    diff_img[off] = 255;
                    diff_img[off + 1] = 0;
                    diff_img[off + 2] = 0;
                    diff_img[off + 3] = 255;
                } else {
                    // Dimmed reference pixel
                    diff_img[off] = reference_raw[off] >> 2;
                    diff_img[off + 1] = reference_raw[off + 1] >> 2;
                    diff_img[off + 2] = reference_raw[off + 2] >> 2;
                    diff_img[off + 3] = reference_raw[off + 3];
                }
            }

            let ratio = diff_pixels as f64 / pixel_count as f64;
            let diff_path = base.join("diff").join(suite).join("native").join(test).join(&file_name);
            if ratio > max_diff_ratio {
                // Save diff image for failing snapshots only.
                std::fs::create_dir_all(diff_path.parent().unwrap()).unwrap();
                let diff_rgba: image::RgbaImage =
                    image::ImageBuffer::from_raw(self.width, self.height, diff_img)
                        .expect("Failed to create diff image");
                diff_rgba.save(&diff_path).unwrap();
                return Err(format!(
                    "Snapshot '{}' differs from reference: {:.4}% pixels changed \
                     (max channel diff: {}, threshold: {:.4}%)\n  \
                     actual: {}\n  reference: {}",
                    name, ratio * 100.0, max_diff, max_diff_ratio * 100.0,
                    output_path.display(), reference_path.display(),
                ));
            }
            // Snapshot passed — remove any stale diff so the report doesn't flag it as changed.
            if diff_path.exists() {
                std::fs::remove_file(&diff_path).ok();
            }
            return Ok(Some(ratio));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod native_movie_validation_tests {
    use super::{validate_native_movie_features, NativeMovieLoadError};

    #[test]
    fn rejects_external_casts_with_paths() {
        let error = validate_native_movie_features(&["shared.cst".to_owned()], 0, 0).unwrap_err();
        let NativeMovieLoadError::Unsupported(message) = error else {
            panic!("external cast was not classified as unsupported");
        };
        assert!(message.contains("shared.cst"));
    }

    #[test]
    fn rejects_embedded_flash() {
        let error = validate_native_movie_features(&[], 1, 0).unwrap_err();
        assert!(matches!(error, NativeMovieLoadError::Unsupported(message) if message.contains("Flash")));
    }

    #[test]
    fn rejects_javascript_lingo_scripts() {
        let error = validate_native_movie_features(&[], 0, 1).unwrap_err();
        assert!(matches!(error, NativeMovieLoadError::Unsupported(message) if message.contains("JavaScript")));
    }
}

#[cfg(test)]
mod native_global_validation_tests {
    use std::collections::{HashSet, VecDeque};

    use super::{NativeGlobalReadError, TestPlayer, strict_native_global_value};
    use crate::director::lingo::datum::{Datum, DatumType};
    use crate::player::DatumRef;

    #[test]
    fn rejects_recursive_global_graphs_without_unbounded_walk() {
        let player = TestPlayer::new();
        let result = player
            .runtime
            .with_context(|context| {
                let list = context.player.alloc_datum(Datum::List(
                    DatumType::List,
                    VecDeque::from([DatumRef::Void]),
                    false,
                ));
                if let Datum::List(_, values, _) = context.player.get_datum_mut(&list) {
                    values[0] = list.clone();
                }
                let mut active = HashSet::new();
                let mut nodes = 0;
                strict_native_global_value(
                    context.player,
                    context.symbols,
                    &list,
                    0,
                    &mut nodes,
                    &mut active,
                )
            })
            .expect("test player must remain live")
            .unwrap_err();
        assert!(matches!(
            result,
            NativeGlobalReadError::Unsupported {
                datum_type: "container",
                reason: Some("cycle")
            }
        ));
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod native_lifecycle_tests {
    use super::*;
    use crate::rendering::{snapshot_native_for_owner, snapshot_native_fresh_for_owner};

    const PROBE_MOVIE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/native_director_probe.dcr"
    );

    #[derive(Clone, Debug, PartialEq)]
    struct LifecycleRecord {
        initial_frame: u32,
        initial_frame_state: StaticDatum,
        initial_input_h: StaticDatum,
        initial_input_v: StaticDatum,
        input_frame: u32,
        input_frame_state: StaticDatum,
        input_h: StaticDatum,
        input_v: StaticDatum,
        at_100: NativeAdvanceReport,
        at_100_frame: u32,
        at_100_frame_state: StaticDatum,
        at_300: NativeAdvanceReport,
        at_300_frame: u32,
        at_300_frame_state: StaticDatum,
        before_rgba: Vec<u8>,
        input_rgba: Vec<u8>,
        final_rgba: Vec<u8>,
    }

    fn expected_probe_rgba(left: u32) -> Vec<u8> {
        let mut data = Vec::with_capacity(32 * 32 * 4);
        for y in 0..32 {
            for x in 0..32 {
                if (left..left + 24).contains(&x) && (4..28).contains(&y) {
                    data.extend_from_slice(&[0, 0, 0, 255]);
                } else {
                    data.extend_from_slice(&[255, 255, 255, 255]);
                }
            }
        }
        data
    }

    fn snapshot_data(player: &TestPlayer) -> Vec<u8> {
        let snapshot = StageSnapshot::from_output(player.snapshot_stage());
        assert_eq!((snapshot.width, snapshot.height), (32, 32));
        assert_eq!(snapshot.data.len(), 4096);
        snapshot.data
    }

    fn write_iteration_artifacts(
        evidence_dir: Option<&Path>,
        iteration: usize,
        record: &LifecycleRecord,
    ) {
        let Some(evidence_dir) = evidence_dir else {
            return;
        };
        std::fs::create_dir_all(evidence_dir).unwrap();
        let stem = format!("iteration-{iteration}");
        std::fs::write(evidence_dir.join(format!("{stem}-before.rgba")), &record.before_rgba)
            .unwrap();
        std::fs::write(evidence_dir.join(format!("{stem}-input.rgba")), &record.input_rgba)
            .unwrap();
        std::fs::write(evidence_dir.join(format!("{stem}-final.rgba")), &record.final_rgba)
            .unwrap();
        let state = format!(
            "schema=NATIVE_LIFECYCLE_V1\n\
             initial_frame={}\n\
             initial_frame_state={:?}\n\
             initial_input_h={:?}\n\
             initial_input_v={:?}\n\
             input_frame={}\n\
             input_frame_state={:?}\n\
             input_h={:?}\n\
             input_v={:?}\n\
             at_100_frames={}\n\
             at_100_timeouts={}\n\
             at_100_frame={}\n\
             at_100_frame_state={:?}\n\
             at_300_frames={}\n\
             at_300_timeouts={}\n\
             at_300_frame={}\n\
             at_300_frame_state={:?}\n\
             before_rgba_bytes={}\n\
             input_rgba_bytes={}\n\
             final_rgba_bytes={}\n",
            record.initial_frame,
            record.initial_frame_state,
            record.initial_input_h,
            record.initial_input_v,
            record.input_frame,
            record.input_frame_state,
            record.input_h,
            record.input_v,
            record.at_100.frames,
            record.at_100.timeouts,
            record.at_100_frame,
            record.at_100_frame_state,
            record.at_300.frames,
            record.at_300.timeouts,
            record.at_300_frame,
            record.at_300_frame_state,
            record.before_rgba.len(),
            record.input_rgba.len(),
            record.final_rgba.len(),
        );
        std::fs::write(evidence_dir.join(format!("{stem}-state.txt")), state).unwrap();
        println!(
            "NATIVE_LIFECYCLE_V1 iteration={iteration} initial_frame={} initial_frame_state={:?} initial_input_h={:?} initial_input_v={:?} input_frame={} input_frame_state={:?} input_h={:?} input_v={:?} at_100={:?} at_300={:?} final_frame={} final_frame_state={:?} rgba_bytes=4096/4096/4096",
            record.initial_frame,
            record.initial_frame_state,
            record.initial_input_h,
            record.initial_input_v,
            record.input_frame,
            record.input_frame_state,
            record.input_h,
            record.input_v,
            record.at_100,
            record.at_300,
            record.at_300_frame,
            record.at_300_frame_state,
        );
    }

    async fn run_iteration(iteration: usize, evidence_dir: Option<&Path>) -> LifecycleRecord {
        let mut player = TestPlayer::new();
        player.load_movie(PROBE_MOVIE).await;
        player.init_movie_at(0).await;
        assert_eq!(player.try_advance_to(0).await.unwrap(), NativeAdvanceReport::default());

        let initial_frame = player.current_frame();
        let initial_frame_state = player.eval_datum("value(\"frameState\")").await.unwrap();
        let initial_input_h = player.eval_datum("value(\"inputH\")").await.unwrap();
        let initial_input_v = player.eval_datum("value(\"inputV\")").await.unwrap();
        assert_eq!(initial_frame, 1);
        assert_eq!(initial_frame_state, StaticDatum::Int(2));
        assert_eq!(initial_input_h, StaticDatum::Void);
        assert_eq!(initial_input_v, StaticDatum::Void);
        let before_rgba = snapshot_data(&player);
        assert_eq!(before_rgba, expected_probe_rgba(4));

        player
            .eval_datum("timeout().new(\"timerA\", 100, #timerA)")
            .await
            .unwrap();
        player
            .eval_datum("timeout().new(\"timerB\", 100, #timerB)")
            .await
            .unwrap();
        assert_eq!(
            player
                .runtime
                .with_context(|context| context.player.timeout_manager.timeouts.len()),
            Some(2)
        );
        assert!(player.take_native_timeout_host_unsupported().is_empty());

        player.native_mouse_down(8, 8).await;
        let input_frame = player.current_frame();
        let input_frame_state = player.eval_datum("value(\"frameState\")").await.unwrap();
        let input_h = player.eval_datum("value(\"inputH\")").await.unwrap();
        let input_v = player.eval_datum("value(\"inputV\")").await.unwrap();
        assert_eq!(input_frame, initial_frame);
        assert_eq!(input_frame_state, initial_frame_state);
        assert_eq!(input_h, StaticDatum::Int(8));
        assert_eq!(input_v, StaticDatum::Int(8));
        let input_rgba = snapshot_data(&player);
        assert_eq!(input_rgba, expected_probe_rgba(8));
        assert!(player.take_native_timeout_host_unsupported().is_empty());

        let at_100 = player.advance_to(100).await;
        let at_100_frame = player.current_frame();
        let at_100_frame_state = player.eval_datum("value(\"frameState\")").await.unwrap();
        assert_eq!(at_100, NativeAdvanceReport { frames: 3, timeouts: 2 });
        assert_eq!(at_100_frame, 1);
        assert_eq!(at_100_frame_state, StaticDatum::Int(141));
        assert!(player.take_native_timeout_host_unsupported().is_empty());

        let at_300 = player.advance_to(300).await;
        let at_300_frame = player.current_frame();
        let at_300_frame_state = player.eval_datum("value(\"frameState\")").await.unwrap();
        assert_eq!(at_300, NativeAdvanceReport { frames: 6, timeouts: 1 });
        assert_eq!(at_300_frame, 1);
        assert_eq!(at_300_frame_state, StaticDatum::Int(157));
        assert!(player.take_native_timeout_host_unsupported().is_empty());
        let final_rgba = snapshot_data(&player);
        assert_eq!(final_rgba, expected_probe_rgba(4));

        let session = player.runtime.session();
        let player_id = player.runtime.player_id();
        let owner = player.runtime.owner().clone();
        let mut stale_frame_pump = NativeFramePump::new(session.clone(), player_id, owner.clone());
        let stale_input_pump = NativeInputPump::new(session.clone(), player_id, owner.clone());
        let weak_presentation = Rc::downgrade(&player.native_presentation);
        let command_tx = player.runtime.command_tx();
        let (command_future, completer) = ManualFuture::new();
        command_tx
            .send(PlayerVMExecutionItem {
                command: PlayerVMCommand::MouseDown((0, 0)),
                completer: Some(completer),
            })
            .await
            .unwrap();

        let record = LifecycleRecord {
            initial_frame,
            initial_frame_state,
            initial_input_h,
            initial_input_v,
            input_frame,
            input_frame_state,
            input_h,
            input_v,
            at_100,
            at_100_frame,
            at_100_frame_state,
            at_300,
            at_300_frame,
            at_300_frame_state,
            before_rgba,
            input_rgba,
            final_rgba,
        };
        write_iteration_artifacts(evidence_dir, iteration, &record);

        drop(player);
        assert!(weak_presentation.upgrade().is_none());
        assert!(session.borrow().native_presentation(player_id).is_none());
        assert!(session.borrow_mut().with_player(player_id, |_| ()).is_none());
        assert!(session.borrow_mut().take_timeout_host_actions().is_empty());
        assert!(session.borrow_mut().take_native_timeout_host_unsupported().is_empty());
        assert!(
            async_std::future::timeout(Duration::from_secs(1), command_future)
                .await
                .is_ok(),
            "retired command completer hung"
        );
        assert!(stale_frame_pump
            .advance_to(301)
            .await
            .is_err());
        assert!(stale_input_pump.mouse_down(0, 0).await.is_err());
        assert!(snapshot_native_for_owner(&session, player_id, &owner).is_err());
        assert!(snapshot_native_fresh_for_owner(&session, player_id, &owner).is_err());
        record
    }

    #[test]
    fn native_test_player_lifecycle() {
        run_test(async {
            let evidence_dir = std::env::var_os("NATIVE_LIFECYCLE_EVIDENCE_DIR").map(PathBuf::from);
            let mut records = Vec::new();
            for iteration in 0..3 {
                records.push(run_iteration(iteration, evidence_dir.as_deref()).await);
            }
            assert!(records.windows(2).all(|pair| pair[0] == pair[1]));
        });
    }
}
