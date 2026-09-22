use std::collections::HashSet;
use std::{cell::{Cell, RefCell}, sync::Mutex};
use std::{
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use async_std::channel;
use manual_future::ManualFuture;
use rand::SeedableRng;

use crate::director::file::read_director_file_bytes;
use crate::director::lingo::datum::Datum;
pub use crate::director::static_datum::StaticDatum;
use crate::native_parity_worker::QualifiedExternalCasts;
use crate::player::testing_shared::HarnessRuntime;
pub use crate::player::testing_shared::{SnapshotOutput, TestHarness};
use crate::player::{
    PlayerVMExecutionItem,
    commands::{PlayerVMCommand, run_command_loop},
    allocator::ScriptInstanceAllocatorTrait,
    host_events::{NativePlayerNotification, NativePlayerNotificationSink, NativePlayerNotificationSinkRef},
    ownership::OwnerKey,
    session::{
        NativeAdvanceReport, NativeFramePump, NativeInputPump, RuntimeSessionHandle,
    },
    NativeFlashActionSummary, NativeFlashBindingObservation,
};
#[cfg(not(target_arch = "wasm32"))]
use crate::player::session::NativeFlashCallbackObservation;

/// Global lock to ensure only one TestPlayer runs at a time.
/// The player uses global mutable statics, so tests must be serialized.
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// Test-only admission held by the dual-host ownership experiment. The guard
/// stays alive while both admitted players run, so unrelated TestPlayer tests
/// cannot enter the process-wide mutable state concurrently.
#[cfg(test)]
pub(crate) struct TestPlayerAdmission {
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl TestPlayerAdmission {
    pub(crate) fn acquire() -> Rc<Self> {
        let lock = match TEST_LOCK.lock() {
            Ok(lock) => lock,
            Err(poisoned) => poisoned.into_inner(),
        };
        Rc::new(Self { _lock: lock })
    }
}

enum TestPlayerAdmissionState {
    Exclusive(std::sync::MutexGuard<'static, ()>),
    #[cfg(test)]
    Shared(Rc<TestPlayerAdmission>),
}

/// Bounded native parity adapter. The production sink acknowledges each DTO
/// synchronously and retains only delivery metadata; tests add a recording
/// side channel to verify FIFO delivery without changing production memory.
struct NativeParityNotificationSink {
    delivered: Cell<u64>,
    last_owner: Cell<Option<OwnerKey>>,
    #[cfg(test)]
    recorded: RefCell<Vec<NativePlayerNotification>>,
}

impl NativeParityNotificationSink {
    fn new() -> Self {
        Self {
            delivered: Cell::new(0),
            last_owner: Cell::new(None),
            #[cfg(test)]
            recorded: RefCell::new(Vec::new()),
        }
    }
}

impl NativePlayerNotificationSink for NativeParityNotificationSink {
    fn accept(&self, notification: &NativePlayerNotification) {
        self.delivered.set(self.delivered.get().saturating_add(1));
        self.last_owner.set(Some(notification.owner.key()));
        #[cfg(test)]
        self.recorded.borrow_mut().push(notification.clone());
    }
}

/// Native test harness. Wraps the global DirPlayer for in-memory testing.
pub struct TestPlayer {
    _tx: channel::Sender<PlayerVMExecutionItem>,
    _admission: TestPlayerAdmissionState,
    runtime: HarnessRuntime,
    native_presentation: Rc<crate::rendering::NativePresentationPolicy>,
    native_flash: Rc<std::cell::RefCell<crate::native_flash::NativeFlashHost>>,
    native_notification_observer: Rc<NativeParityNotificationSink>,
    native_frame_pump: NativeFramePump,
    native_input_pump: NativeInputPump,
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct NativeTeardownWitness {
    pub(crate) session: RuntimeSessionHandle,
    pub(crate) player_id: crate::player::session::PlayerId,
    pub(crate) owner: crate::player::ownership::OwnerToken,
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

/// Typed values accepted by the native worker's owner-bound Invoke bridge.
/// The conversion to Datum happens only while the owning player and symbol
/// table are borrowed together.
#[derive(Debug)]
pub(crate) enum NativeInvokeArgument {
    Int(i32),
    Float(f64),
    String(String),
    Symbol(String),
    Void,
}

#[derive(Debug, PartialEq)]
pub(crate) struct NativeInitObservation {
    pub(crate) next_frame: Option<u32>,
    pub(crate) flash_bindings: Vec<NativeFlashBindingObservation>,
    pub(crate) pending_flash_actions: Vec<NativeFlashActionSummary>,
}

#[derive(Debug, PartialEq)]
pub(crate) struct NativeInputSpriteObservation {
    pub(crate) sprite: i16,
    pub(crate) member_ref: Option<(u32, u32)>,
    pub(crate) member_name: Option<String>,
}

#[derive(Debug, PartialEq)]
pub(crate) struct NativeSpriteGeometryObservation {
    pub(crate) draw_order: usize,
    pub(crate) sprite: i16,
    pub(crate) name: String,
    pub(crate) member_ref: Option<(u32, u32)>,
    pub(crate) member_name: Option<String>,
    pub(crate) member_type: Option<String>,
    pub(crate) member_text: Option<String>,
    pub(crate) behavior_instances: Vec<NativeBehaviorObservation>,
    pub(crate) visible: bool,
    pub(crate) puppet: bool,
    pub(crate) loc: [i32; 3],
    pub(crate) size: [i32; 2],
    pub(crate) rect: [i32; 4],
    pub(crate) ink: i32,
    pub(crate) blend: i32,
    pub(crate) rotation: f64,
    pub(crate) skew: f64,
    pub(crate) stretch: i32,
    pub(crate) entered: bool,
    pub(crate) exited: bool,
    pub(crate) has_size_tweened: bool,
    pub(crate) has_size_changed: bool,
}

#[derive(Debug, PartialEq)]
pub(crate) struct NativeBehaviorObservation {
    pub(crate) instance_id: u32,
    pub(crate) script_ref: Option<(u32, u32)>,
    pub(crate) script_name: Option<String>,
    pub(crate) script_type: Option<String>,
    pub(crate) begin_sprite_called: Option<bool>,
}

#[derive(Debug, PartialEq)]
pub(crate) struct NativeInputObservation {
    pub(crate) owner: OwnerKey,
    pub(crate) pointer: Option<(i32, i32)>,
    pub(crate) mouse_down: bool,
    pub(crate) captured_sprite: Option<NativeInputSpriteObservation>,
    pub(crate) click_on_sprite: Option<NativeInputSpriteObservation>,
    pub(crate) hovered_sprites: Vec<NativeInputSpriteObservation>,
}

#[derive(Debug)]
pub(crate) enum NativeGlobalReadError {
    Unsupported {
        datum_type: &'static str,
        reason: Option<&'static str>,
    },
    Runtime(crate::player::ScriptError),
}

#[derive(Debug)]
pub(crate) enum NativeInvokeError {
    Runtime(crate::player::ScriptError),
    Returned(NativeGlobalReadError),
    UnsupportedContinuation(&'static str),
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

        Self::try_new_with_admission(TestPlayerAdmissionState::Exclusive(lock))
    }

    #[cfg(test)]
    pub(crate) fn try_new_with_test_admission(
        admission: Rc<TestPlayerAdmission>,
    ) -> Result<Self, crate::player::ScriptError> {
        Self::try_new_with_admission(TestPlayerAdmissionState::Shared(admission))
    }

    fn try_new_with_admission(
        admission: TestPlayerAdmissionState,
    ) -> Result<Self, crate::player::ScriptError> {

        let (tx, rx) = channel::unbounded();

        let runtime = HarnessRuntime::try_new(tx.clone())?;
        let native_presentation = Rc::new(crate::rendering::NativePresentationPolicy::new());
        runtime
            .session()
            .borrow_mut()
            .bind_native_presentation(runtime.player_id(), &native_presentation);
        let native_flash = Rc::new(std::cell::RefCell::new(
            crate::native_flash::NativeFlashHost::new(),
        ));
        runtime
            .session()
            .borrow_mut()
            .bind_native_flash(runtime.player_id(), &native_flash);
        let native_notification_observer = Rc::new(NativeParityNotificationSink::new());
        let native_notification_sink: NativePlayerNotificationSinkRef =
            native_notification_observer.clone();
        runtime
            .session()
            .borrow_mut()
            .bind_native_player_notification_sink(
                runtime.player_id(),
                runtime.owner(),
                &native_notification_sink,
            )?;
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
            _admission: admission,
            runtime,
            native_presentation,
            native_flash,
            native_notification_observer,
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

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn native_pcm_snapshot_quiet(&self) -> Vec<[f32; 2]> {
        self.native_frame_pump.native_pcm_snapshot()
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
    pub fn take_native_timeout_host_unsupported(
        &mut self,
    ) -> Vec<crate::js_api::TimeoutHostDispatch> {
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

    pub(crate) async fn try_native_mouse_move(
        &self,
        x: i32,
        y: i32,
    ) -> Result<(), crate::player::ScriptError> {
        self.native_input_pump.mouse_move(x, y).await
    }

    pub(crate) async fn try_native_mouse_up(
        &self,
        x: i32,
        y: i32,
    ) -> Result<(), crate::player::ScriptError> {
        self.native_input_pump.mouse_up(x, y).await
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

    pub(crate) fn configure_native_presentation(
        &self,
        viewport: Option<crate::rendering::NativePresentationViewport>,
    ) -> Result<(), crate::player::ScriptError> {
        let (stage_width, stage_height) = self
            .runtime
            .with_context(|context| (context.player.movie.rect.width(), context.player.movie.rect.height()))
            .ok_or_else(|| {
                crate::player::ScriptError::new("harness player was replaced".to_owned())
            })?;
        self.native_presentation
            .set_viewport(viewport, stage_width, stage_height)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn install_native_flash_callback_observer(
        &mut self,
        observer: Rc<RefCell<Vec<NativeFlashCallbackObservation>>>,
    ) {
        self.native_frame_pump
            .set_native_flash_callback_observer(Some(observer));
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn native_teardown_witness(&self) -> NativeTeardownWitness {
        NativeTeardownWitness {
            session: self.runtime.session(),
            player_id: self.runtime.player_id(),
            owner: self.runtime.owner().clone(),
        }
    }

    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) fn native_flash_handle(
        &self,
    ) -> Rc<std::cell::RefCell<crate::native_flash::NativeFlashHost>> {
        self.native_flash.clone()
    }

    pub(crate) async fn load_movie_quiet(
        &mut self,
        path: &str,
        data_bytes: Vec<u8>,
    ) -> Result<(), NativeMovieLoadError> {
        self.load_movie_quiet_inner(path, data_bytes, None).await
    }

    pub(crate) async fn load_movie_quiet_with_qualified_casts(
        &mut self,
        path: &str,
        data_bytes: Vec<u8>,
        qualification: &QualifiedExternalCasts,
    ) -> Result<(), NativeMovieLoadError> {
        self.load_movie_quiet_inner(path, data_bytes, Some(qualification))
            .await
    }

    async fn load_movie_quiet_inner(
        &mut self,
        path: &str,
        data_bytes: Vec<u8>,
        qualification: Option<&QualifiedExternalCasts>,
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
        let dir_file =
            read_director_file_bytes(&data_bytes, &file_name, &base_url).map_err(|error| {
                NativeMovieLoadError::Runtime(crate::player::ScriptError::new(error))
            })?;
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
        validate_native_movie_features(
            &external_casts,
            embedded_flash,
            javascript_scripts,
            qualification,
        )?;
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
        .map_err(NativeMovieLoadError::Runtime)?;
        if let Some(qualification) = qualification {
            self.apply_qualified_external_casts(qualification)?;
        }
        Ok(())
    }

    fn apply_qualified_external_casts(
        &mut self,
        qualification: &QualifiedExternalCasts,
    ) -> Result<(), NativeMovieLoadError> {
        use crate::player::cast_lib::CastLibState;
        use crate::player::cast_manager::{CastPreloadReason, CastPreloadState};

        let requests = self
            .runtime
            .session()
            .borrow_mut()
            .prepare_cast_loads(self.runtime.player_id(), CastPreloadReason::MovieLoaded);
        if requests.len() != qualification.len() {
            return Err(NativeMovieLoadError::Runtime(
                crate::player::ScriptError::new(format!(
                    "validated external cast count {} does not match preload request count {}",
                    qualification.len(),
                    requests.len()
                )),
            ));
        }
        let mut seen = HashSet::new();
        for request in requests {
            let (key, bytes) = qualification
                .entry_for_request(request.requested_url())
                .ok_or_else(|| {
                    NativeMovieLoadError::Runtime(crate::player::ScriptError::new(format!(
                        "preload request '{}' has no validated alias",
                        request.requested_url()
                    )))
                })?;
            if !seen.insert(key.to_owned()) {
                return Err(NativeMovieLoadError::Runtime(
                    crate::player::ScriptError::new(format!(
                        "duplicate preload request for validated alias '{key}'"
                    )),
                ));
            }
            let applied = self.runtime.session().borrow_mut().apply_cast_load(
                request.complete(request.requested_url().to_owned(), Ok(bytes.to_vec())),
            );
            if !applied {
                return Err(NativeMovieLoadError::Runtime(
                    crate::player::ScriptError::new(format!(
                        "owner-qualified preload application failed for alias '{key}'"
                    )),
                ));
            }
        }
        if seen.len() != qualification.len() {
            return Err(NativeMovieLoadError::Runtime(
                crate::player::ScriptError::new(
                    "validated external cast aliases were not all requested".to_owned(),
                ),
            ));
        }
        let ready = self.runtime.with_context(|context| {
            let mut external_casts = context
                .player
                .movie
                .cast_manager
                .casts
                .iter()
                .filter(|cast| cast.is_external);
            if qualification.len() == 0 {
                return external_casts.count() == 0;
            }
            context.player.movie.cast_manager.preload_state == CastPreloadState::Ready
                && external_casts.all(|cast| cast.state == CastLibState::Loaded)
        });
        if ready != Some(true) {
            return Err(NativeMovieLoadError::Runtime(
                crate::player::ScriptError::new(
                    "qualified external casts did not reach the ready preload barrier".to_owned(),
                ),
            ));
        }
        Ok(())
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

    pub(crate) fn current_label_quiet(&self) -> Option<String> {
        self.runtime
            .with_context(|context| {
                context
                    .player
                    .movie
                    .score
                    .frame_labels
                    .iter()
                    .filter(|label| label.frame_num <= context.player.movie.current_frame as i32)
                    .max_by_key(|label| label.frame_num)
                    .map(|label| label.label.clone())
            })
            .flatten()
    }

    pub(crate) fn native_init_state_quiet(
        &self,
    ) -> Result<NativeInitObservation, crate::player::ScriptError> {
        if !self.runtime.owner_valid() {
            return Err(crate::player::ScriptError::new(
                "native init inspection owner is stale".to_owned(),
            ));
        }
        let observation = self
            .runtime
            .with_context(|context| NativeInitObservation {
                next_frame: context.player.next_frame,
                flash_bindings: context.player.native_flash_bindings(),
                pending_flash_actions: context.player.pending_flash_action_summaries(),
            })
            .ok_or_else(|| crate::player::ScriptError::new("native init inspection owner changed".to_owned()))?;
        if !self.runtime.owner_valid() {
            return Err(crate::player::ScriptError::new(
                "native init inspection owner changed".to_owned(),
            ));
        }
        Ok(observation)
    }

    pub(crate) fn native_input_state_quiet(
        &self,
    ) -> Result<NativeInputObservation, crate::player::ScriptError> {
        if !self.runtime.owner_valid() {
            return Err(crate::player::ScriptError::new(
                "native input inspection owner is stale".to_owned(),
            ));
        }
        let observation = self
            .runtime
            .with_context(|context| {
                let player = context.player;
                let sprite_observation = |sprite: i16| {
                    let member_ref = player
                        .movie
                        .score
                        .get_sprite(sprite)
                        .and_then(|sprite| sprite.member.clone());
                    let member_name = member_ref.as_ref().and_then(|member_ref| {
                        player
                            .movie
                            .cast_manager
                            .find_member_by_ref(member_ref)
                            .map(|member| member.name.clone())
                    });
                    NativeInputSpriteObservation {
                        sprite,
                        member_ref: member_ref
                            .map(|member_ref| (member_ref.cast_lib as u32, member_ref.cast_member as u32)),
                        member_name,
                    }
                };
                NativeInputObservation {
                    owner: player.owner.key(),
                    pointer: Some(player.mouse_loc),
                    mouse_down: player.movie.mouse_down,
                    captured_sprite: (player.mouse_down_sprite > 0)
                        .then(|| sprite_observation(player.mouse_down_sprite)),
                    click_on_sprite: (player.click_on_sprite > 0)
                        .then(|| sprite_observation(player.click_on_sprite)),
                    hovered_sprites: player
                        .hovered_sprites
                        .iter()
                        .copied()
                        .map(sprite_observation)
                        .collect(),
                }
            })
            .ok_or_else(|| {
                crate::player::ScriptError::new("native input inspection owner changed".to_owned())
            })?;
        if !self.runtime.owner_valid() {
            return Err(crate::player::ScriptError::new(
                "native input inspection owner changed".to_owned(),
            ));
        }
        Ok(observation)
    }

    pub(crate) fn native_sprite_geometry_quiet(
        &self,
    ) -> Result<Vec<NativeSpriteGeometryObservation>, crate::player::ScriptError> {
        if !self.runtime.owner_valid() {
            return Err(crate::player::ScriptError::new(
                "native sprite geometry inspection owner is stale".to_owned(),
            ));
        }
        let observation = self
            .runtime
            .with_context(|context| {
                let player = context.player;
                player
                    .movie
                    .score
                    .get_sorted_channels(player.movie.current_frame)
                    .into_iter()
                    .enumerate()
                    .map(|(draw_order, channel)| {
                        let sprite = &channel.sprite;
                        let member_ref = sprite.member.clone();
                        let member = member_ref.as_ref().and_then(|member_ref| {
                            player
                                .movie
                                .cast_manager
                                .find_member_by_ref(member_ref)
                        });
                        let member_name = member.map(|member| member.name.clone());
                        let member_type = member.map(|member| member.member_type.type_string().to_owned());
                        let member_text = member.and_then(|member| match &member.member_type {
                            crate::player::cast_member::CastMemberType::Field(field) => Some(field.text.clone()),
                            crate::player::cast_member::CastMemberType::Text(text) => Some(text.text.clone()),
                            crate::player::cast_member::CastMemberType::Button(button) => Some(button.field.text.clone()),
                            _ => None,
                        });
                        let behavior_instances = sprite
                            .script_instance_list
                            .iter()
                            .map(|instance_ref| {
                                let instance = player.allocator.get_script_instance_opt(instance_ref);
                                let script_ref = instance.map(|instance| {
                                    (instance.script.cast_lib as u32, instance.script.cast_member as u32)
                                });
                                let script_member = instance.and_then(|instance| {
                                    player.movie.cast_manager.find_member_by_ref(&instance.script)
                                });
                                NativeBehaviorObservation {
                                    instance_id: instance_ref.id(),
                                    script_ref,
                                    script_name: script_member.map(|member| member.name.clone()),
                                    script_type: script_member
                                        .and_then(|member| member.member_type.as_script())
                                        .map(|script| format!("{:?}", script.script_type)),
                                    begin_sprite_called: instance.map(|instance| instance.begin_sprite_called),
                                }
                            })
                            .collect();
                        let rect =
                            crate::player::score::get_concrete_sprite_rect(player, sprite);
                        NativeSpriteGeometryObservation {
                            draw_order,
                            sprite: sprite.number as i16,
                            name: sprite.name.clone(),
                            member_ref: member_ref
                                .map(|member_ref| {
                                    (member_ref.cast_lib as u32, member_ref.cast_member as u32)
                                }),
                            member_name,
                            member_type,
                            member_text,
                            behavior_instances,
                            visible: sprite.visible,
                            puppet: sprite.puppet,
                            loc: [sprite.loc_h, sprite.loc_v, sprite.loc_z],
                            size: [sprite.width, sprite.height],
                            rect: [rect.left, rect.top, rect.right, rect.bottom],
                            ink: sprite.ink,
                            blend: sprite.blend,
                            rotation: sprite.rotation,
                            skew: sprite.skew,
                            stretch: sprite.stretch,
                            entered: sprite.entered,
                            exited: sprite.exited,
                            has_size_tweened: sprite.has_size_tweened,
                            has_size_changed: sprite.has_size_changed,
                        }
                    })
                    .collect()
            })
            .ok_or_else(|| {
                crate::player::ScriptError::new(
                    "native sprite geometry inspection owner changed".to_owned(),
                )
            })?;
        if !self.runtime.owner_valid() {
            return Err(crate::player::ScriptError::new(
                "native sprite geometry inspection owner changed".to_owned(),
            ));
        }
        Ok(observation)
    }

    pub(crate) fn native_flash_snapshots_quiet(
        &self,
    ) -> Result<Vec<crate::native_flash::NativeFlashSnapshot>, crate::player::ScriptError> {
        if !self.runtime.owner_valid() {
            return Err(crate::player::ScriptError::new(
                "native Flash inspection owner is stale".to_owned(),
            ));
        }
        let weak_host = {
            let session = self.runtime.session();
            let session = session.borrow();
            session
                .native_flash(self.runtime.player_id())
                .ok_or_else(|| {
                    crate::player::ScriptError::new(
                        "native Flash inspection host is unavailable".to_owned(),
                    )
                })?
        };
        let host = weak_host.upgrade().ok_or_else(|| {
            crate::player::ScriptError::new("native Flash inspection host is stale".to_owned())
        })?;
        let snapshots = host.borrow().snapshots()?;
        if !self.runtime.owner_valid() {
            return Err(crate::player::ScriptError::new(
                "native Flash inspection owner changed".to_owned(),
            ));
        }
        Ok(snapshots)
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
                        context
                            .symbols
                            .lower(symbol)
                            .is_ok_and(|name| name == lower_name)
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

    /// Invoke a validated global through the owning player's command queue.
    /// The normal command loop adopts and resumes any owner-bound internal
    /// continuation before the wire operation receives its terminal result.
    pub(crate) async fn native_invoke_global_quiet(
        &self,
        function: &str,
        arguments: Vec<NativeInvokeArgument>,
    ) -> Result<NativeGlobalValue, NativeInvokeError> {
        if !self.runtime.owner_valid() {
            return Err(NativeInvokeError::Runtime(crate::player::ScriptError::new(
                "native invocation owner is stale".to_owned(),
            )));
        }
        let Some((handler, args)) = self.runtime.with_context(|context| {
            let handler = context.symbols.intern(function);
            let args = arguments
                .into_iter()
                .map(|argument| {
                    let datum = match argument {
                        NativeInvokeArgument::Int(value) => Datum::Int(value),
                        NativeInvokeArgument::Float(value) => Datum::Float(value),
                        NativeInvokeArgument::String(value) => Datum::String(value),
                        NativeInvokeArgument::Symbol(value) => {
                            Datum::Symbol(context.symbols.intern(&value))
                        }
                        NativeInvokeArgument::Void => Datum::Void,
                    };
                    context.player.alloc_datum(datum)
                })
                .collect::<Vec<_>>();
            (handler, args)
        }) else {
            return Err(NativeInvokeError::Runtime(crate::player::ScriptError::new(
                "native invocation owner was replaced while preparing arguments".to_owned(),
            )));
        };
        let command_tx = self.runtime.command_tx();
        let (future, completer) = ManualFuture::new();
        command_tx
            .send(PlayerVMExecutionItem {
                command: PlayerVMCommand::InvokeGlobal { handler, args },
                completer: Some(completer),
            })
            .await
            .map_err(|_| {
                NativeInvokeError::Runtime(crate::player::ScriptError::new(
                    "native invocation owner command queue closed".to_owned(),
                ))
            })?;
        let value = future.await.map_err(NativeInvokeError::Runtime)?;
        let result = self
            .runtime
            .with_context(|context| {
                let mut active = HashSet::new();
                let mut nodes = 0;
                strict_native_global_value(
                    context.player,
                    context.symbols,
                    &value,
                    0,
                    &mut nodes,
                    &mut active,
                )
            })
            .ok_or_else(|| {
                NativeInvokeError::Runtime(crate::player::ScriptError::new(
                    "native invocation owner was replaced before result serialization".to_owned(),
                ))
            })?
            .map_err(NativeInvokeError::Returned)?;
        if !self.runtime.owner_valid() {
            return Err(NativeInvokeError::Runtime(crate::player::ScriptError::new(
                "native invocation owner changed before returning its result".to_owned(),
            )));
        }
        Ok(result)
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
    let datum = player.allocator.try_get_datum(value_ref).ok_or_else(|| {
        NativeGlobalReadError::Runtime(crate::player::ScriptError::new(
            "global contains a foreign or stale datum reference".to_owned(),
        ))
    })?;
    let result = match datum {
        crate::director::lingo::datum::Datum::Int(value) => Ok(NativeGlobalValue::Int(*value)),
        crate::director::lingo::datum::Datum::Float(value) if value.is_finite() => {
            Ok(NativeGlobalValue::Float(*value))
        }
        crate::director::lingo::datum::Datum::Float(_) => Err(NativeGlobalReadError::Unsupported {
            datum_type: "float",
            reason: Some("non_finite"),
        }),
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
    qualification: Option<&QualifiedExternalCasts>,
) -> Result<(), NativeMovieLoadError> {
    if !external_casts.is_empty() && qualification.is_none() {
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
    fn harness_runtime(&self) -> &HarnessRuntime {
        &self.runtime
    }

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

        let data_bytes = std::fs::read(&abs_path)
            .unwrap_or_else(|e| panic!("Failed to read {}: {}", abs_path, e));

        let file_name = Path::new(&abs_path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();

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
        let delay_ms = self
            .runtime
            .with_context(|context| {
                let tempo = context.player.movie.get_effective_tempo();
                if tempo > 0 { 1000 / tempo } else { 33 }
            })
            .unwrap_or(33);
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
        self.runtime
            .session()
            .borrow_mut()
            .unbind_native_flash(self.runtime.player_id());
        self.runtime
            .session()
            .borrow_mut()
            .unbind_native_player_notification_sink(
                self.runtime.player_id(),
                self.runtime.owner(),
            );
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
    let _ =
        log::set_logger(&NATIVE_TEST_LOGGER).map(|()| log::set_max_level(log::LevelFilter::Warn));
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
            SnapshotOutput::Rgba {
                width,
                height,
                data,
            } => StageSnapshot {
                width,
                height,
                data,
            },
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
            encoder,
            img.as_raw(),
            self.width,
            self.height,
            image::ExtendedColorType::Rgba8,
        )
        .expect("Failed to encode PNG");
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
    pub fn assert_snapshot(
        &self,
        snapshot_path: &str,
        name: &str,
        max_diff_ratio: f64,
        pixel_tolerance: u8,
    ) -> Result<Option<f64>, String> {
        let (suite, test) = snapshot_path
            .split_once('/')
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
            let reference_img =
                image::load_from_memory(&reference_data).expect("Failed to decode reference PNG");
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
                let dg = (self.data[off + 1] as i16 - reference_raw[off + 1] as i16).unsigned_abs()
                    as u8;
                let db = (self.data[off + 2] as i16 - reference_raw[off + 2] as i16).unsigned_abs()
                    as u8;
                let da = (self.data[off + 3] as i16 - reference_raw[off + 3] as i16).unsigned_abs()
                    as u8;
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
            let diff_path = base
                .join("diff")
                .join(suite)
                .join("native")
                .join(test)
                .join(&file_name);
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
                    name,
                    ratio * 100.0,
                    max_diff,
                    max_diff_ratio * 100.0,
                    output_path.display(),
                    reference_path.display(),
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
    use super::{NativeMovieLoadError, TestPlayer, run_test, validate_native_movie_features};
    use crate::native_parity_worker::QualifiedExternalCasts;
    use crate::player::cast_lib::{CastLib, CastLibState, CastMemberRef};
    use crate::player::cast_manager::CastPreloadState;
    use crate::player::cast_member::{ButtonMember, ButtonType, CastMember, CastMemberType, FieldMember};
    use crate::player::score::SpriteChannel;
    use crate::rendering::snapshot_native_for_owner;
    use crate::rendering::{native_button_font_attempts, reset_native_button_font_attempts};
    use std::panic::{AssertUnwindSafe, catch_unwind};

    const PROBE_MOVIE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/native_director_probe.dcr"
    );

    #[test]
    fn native_presentation_default_and_viewport_render_fixture() {
        run_test(async {
            let mut player = TestPlayer::try_new().expect("test player construction failed");
            let movie_bytes = std::fs::read(PROBE_MOVIE).expect("probe fixture must exist");
            player
                .load_movie_quiet(PROBE_MOVIE, movie_bytes)
                .await
                .expect("probe fixture must load");

            let (stage_width, stage_height) = player
                .runtime
                .with_context(|context| {
                    (context.player.movie.rect.width(), context.player.movie.rect.height())
                })
                .expect("test player must remain live");
            let owner = player.runtime.owner().clone();
            let full = snapshot_native_for_owner(
                &player.runtime.session(),
                player.runtime.player_id(),
                &owner,
            )
            .expect("default native presentation must render");
            assert_eq!(full.width, stage_width as u16);
            assert_eq!(full.height, stage_height as u16);

            let viewport = crate::rendering::NativePresentationViewport {
                x: 1,
                y: 1,
                width: i64::from(stage_width - 2),
                height: i64::from(stage_height - 2),
            };
            player
                .configure_native_presentation(Some(viewport))
                .expect("fixture viewport must be valid");
            let cropped = snapshot_native_for_owner(
                &player.runtime.session(),
                player.runtime.player_id(),
                &owner,
            )
            .expect("configured native presentation must render");
            let expected = full.crop_rgba(1, 1, (stage_width - 2) as u16, (stage_height - 2) as u16);
            assert_eq!(cropped.width, expected.width);
            assert_eq!(cropped.height, expected.height);
            assert_eq!(cropped.data, expected.data);
        });
    }

    #[test]
    fn native_presentation_button_cull_reaches_font_only_when_visible_or_uncertain() {
        run_test(async {
            let mut player = TestPlayer::try_new().expect("test player construction failed");
            let movie_bytes = std::fs::read(PROBE_MOVIE).expect("probe fixture must exist");
            player
                .load_movie_quiet(PROBE_MOVIE, movie_bytes)
                .await
                .expect("probe fixture must load");
            let owner = player.runtime.owner().clone();
            let (stage_width, stage_height) = player
                .runtime
                .with_context(|context| {
                    (context.player.movie.rect.width(), context.player.movie.rect.height())
                })
                .expect("test player must remain live");
            let viewport = crate::rendering::NativePresentationViewport {
                x: 0,
                y: 0,
                width: i64::from((stage_width / 2).max(1)),
                height: i64::from((stage_height / 2).max(1)),
            };
            player
                .configure_native_presentation(Some(viewport))
                .expect("fixture viewport must be valid");

            let install_button = |player: &TestPlayer, loc_h: i32, loc_v: i32, trails: bool| {
                player
                    .runtime
                    .with_context(|context| {
                        context.player.font_manager.system_font = None;
                        context.player.font_manager.font_cache.clear();
                        let cast = context
                            .player
                            .movie
                            .cast_manager
                            .casts
                            .first_mut()
                            .expect("probe movie must have a cast");
                        let cast_lib = cast.number as i32;
                        let mut field = FieldMember::new();
                        field.text = "EDIT LEVEL".to_owned();
                        field.font = "native-test-font".to_owned();
                        field.width = 20;
                        field.height = 20;
                        field.rect_right = 20;
                        field.rect_bottom = 20;
                        cast.members.insert(
                            999,
                            CastMember::new(
                                999,
                                CastMemberType::Button(ButtonMember {
                                    field,
                                    button_type: ButtonType::PushButton,
                                    hilite: false,
                                    script_id: 0,
                                    member_script_ref: None,
                                }),
                            ),
                        );
                        let mut channel = SpriteChannel::new(1);
                        channel.sprite.member = Some(CastMemberRef {
                            cast_lib,
                            cast_member: 999,
                        });
                        channel.sprite.puppet = true;
                        channel.sprite.loc_h = loc_h;
                        channel.sprite.loc_v = loc_v;
                        channel.sprite.width = 20;
                        channel.sprite.height = 20;
                        channel.sprite.trails = trails;
                        context.player.movie.score.channels.clear();
                        context.player.movie.score.channels.push(SpriteChannel::new(0));
                        context.player.movie.score.channels.push(channel);
                        context.player.movie.score.invalidate_render_channel_cache();
                        assert_eq!(context.player.movie.score.get_sorted_channels(context.player.movie.current_frame).len(), 1);
                        assert!(context.player.movie.cast_manager.find_member_by_ref(&CastMemberRef {
                            cast_lib,
                            cast_member: 999,
                        }).is_some());
                    })
                    .expect("test player must remain live");
            };

            reset_native_button_font_attempts();
            install_button(&player, stage_width.saturating_sub(1), stage_height.saturating_sub(1), false);
            player
                .configure_native_presentation(Some(viewport))
                .expect("fixture viewport must remain valid");
            snapshot_native_for_owner(
                &player.runtime.session(),
                player.runtime.player_id(),
                &owner,
            )
            .expect("disjoint Button must render without native font dispatch");
            assert_eq!(native_button_font_attempts(), 0);

            reset_native_button_font_attempts();
            install_button(&player, 10, 10, false);
            player
                .configure_native_presentation(Some(viewport))
                .expect("fixture viewport must remain valid");
            let intersecting = catch_unwind(AssertUnwindSafe(|| {
                snapshot_native_for_owner(
                    &player.runtime.session(),
                    player.runtime.player_id(),
                    &owner,
                )
            }));
            assert!(intersecting.is_err(), "intersecting Button must surface the existing native font failure");
            assert_eq!(native_button_font_attempts(), 1);

            reset_native_button_font_attempts();
            install_button(&player, stage_width.saturating_sub(1), stage_height.saturating_sub(1), true);
            player
                .configure_native_presentation(Some(viewport))
                .expect("fixture viewport must remain valid");
            let uncertain = catch_unwind(AssertUnwindSafe(|| {
                snapshot_native_for_owner(
                    &player.runtime.session(),
                    player.runtime.player_id(),
                    &owner,
                )
            }));
            assert!(uncertain.is_err(), "uncertain Button must reach the existing native font failure");
            assert_eq!(native_button_font_attempts(), 1);
        });
    }

    #[test]
    fn applies_qualified_casts_through_owner_pipeline_before_movie_init() {
        run_test(async {
            let mut player = TestPlayer::try_new().expect("test player construction failed");
            let movie_bytes = std::fs::read(PROBE_MOVIE).expect("probe fixture must exist");
            player
                .load_movie_quiet(PROBE_MOVIE, movie_bytes.clone())
                .await
                .expect("probe fixture must load");

            player
                .runtime
                .with_context(|context| {
                    context
                        .player
                        .movie
                        .cast_manager
                        .casts
                        .push(CastLib::test_external(2, 0));
                })
                .expect("test player must remain live");
            let movie_dir = std::path::Path::new(PROBE_MOVIE)
                .parent()
                .expect("probe fixture must have a parent");
            let requested_url = url::Url::from_directory_path(movie_dir)
                .expect("probe fixture directory must form a file URL")
                .join("external-2.cct")
                .expect("external cast URL must resolve")
                .to_string();
            let qualification = QualifiedExternalCasts::from_test_entries([(
                "external-2.cct".to_owned(),
                requested_url,
                movie_bytes,
            )]);

            player
                .apply_qualified_external_casts(&qualification)
                .expect("owner-qualified cast application must succeed");

            let state = player
                .runtime
                .with_context(|context| {
                    (
                        context.player.movie.cast_manager.preload_state,
                        context.player.movie.cast_manager.casts[0].state,
                        context.player.movie.cast_manager.casts[1].state,
                    )
                })
                .expect("test player must remain live");
            assert_eq!(state.0, CastPreloadState::Ready);
            assert_eq!(state.1, CastLibState::Loaded);
            assert_eq!(state.2, CastLibState::Loaded);
        });
    }


    #[test]
    fn rejects_external_casts_with_paths() {
        let error =
            validate_native_movie_features(&["shared.cst".to_owned()], 0, 0, None).unwrap_err();
        let NativeMovieLoadError::Unsupported(message) = error else {
            panic!("external cast was not classified as unsupported");
        };
        assert!(message.contains("shared.cst"));
    }

    #[test]
    fn rejects_embedded_flash() {
        let error = validate_native_movie_features(&[], 1, 0, None).unwrap_err();
        assert!(
            matches!(error, NativeMovieLoadError::Unsupported(message) if message.contains("Flash"))
        );
    }

    #[test]
    fn rejects_javascript_lingo_scripts() {
        let error = validate_native_movie_features(&[], 0, 1, None).unwrap_err();
        assert!(
            matches!(error, NativeMovieLoadError::Unsupported(message) if message.contains("JavaScript"))
        );
    }
}

#[cfg(test)]
mod native_flash_observation_tests {
    use super::TestPlayer;
    use crate::player::{FlashActionFence, FlashHostAction, score::SpriteChannel};

    #[test]
    fn native_flash_snapshot_rejects_retired_owner() {
        let mut player = TestPlayer::try_new().expect("test player construction failed");
        player.runtime.retire_current();
        let error = player
            .native_flash_snapshots_quiet()
            .expect_err("retired owner must not expose Flash state");
        assert!(error.message.contains("owner is stale"));
    }

    #[test]
    fn native_init_observation_reports_pending_seek_and_rejects_stale_owner() {
        let mut player = TestPlayer::try_new().expect("test player construction failed");
        player.runtime.with_context(|context| {
            context.player.movie.score.channels =
                (0..=1).map(SpriteChannel::new).collect();
            let generation = context
                .player
                .flash_binding_state
                .borrow_mut()
                .reserve_for_pair(1, 7, 8)
                .expect("Flash binding generation");
            context.player.next_frame = Some(5);
            context
                .player
                .movie
                .score
                .get_sprite_mut(1)
                .flash_asserted_frame = Some(371);
            context.player.flash_host_actions.push(FlashHostAction::Seek {
                fence: FlashActionFence::legacy(
                    context.player.owner.clone(),
                    context.player.flash_binding_state.clone(),
                ),
                host_sprite: 1,
                local_sprite: 1,
                cast_lib: 7,
                cast_member: 8,
                generation,
                frame: 371,
            });
        });

        let observation = player
            .native_init_state_quiet()
            .expect("live owner must expose init state");
        assert_eq!(observation.next_frame, Some(5));
        assert_eq!(
            observation.flash_bindings,
            vec![crate::player::NativeFlashBindingObservation {
                sprite: 1,
                generation: 1,
                cast_lib: 7,
                cast_member: 8,
                asserted_frame: Some(371),
            }]
        );
        assert_eq!(
            observation.pending_flash_actions,
            vec![crate::player::NativeFlashActionSummary {
                kind: "seek",
                sprite: 1,
                generation: 1,
                cast_lib: 7,
                cast_member: 8,
            }]
        );

        player.runtime.retire_current();
        let error = player
            .native_init_state_quiet()
            .expect_err("retired owner must not expose init state");
        assert!(error.message.contains("owner is stale"));
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
    use crate::player::symbols::{builtin::BuiltInSymbol, symbol::Symbol};

    const PROBE_MOVIE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/native_director_probe.dcr"
    );

    fn native_clock_values(player: &TestPlayer) -> (StaticDatum, StaticDatum) {
        player
            .runtime
            .with_context(|context| {
                let ticks = context
                    .player
                    .get_movie_prop(context.symbols, Symbol::builtin(BuiltInSymbol::Ticks))?;
                let milliseconds = context.player.get_movie_prop(
                    context.symbols,
                    Symbol::builtin(BuiltInSymbol::MilliSeconds),
                )?;
                Ok::<_, crate::player::ScriptError>((
                    crate::director::static_datum::static_datum_from_datum_ref(
                        context.player,
                        context.symbols,
                        &ticks,
                    )?,
                    crate::director::static_datum::static_datum_from_datum_ref(
                        context.player,
                        context.symbols,
                        &milliseconds,
                    )?,
                ))
            })
            .expect("test player must remain live")
            .expect("native clock getters must resolve")
    }

    #[test]
    fn native_clock_getters_follow_controlled_time_and_source_ambient_gate() {
        run_test(async {
            let combined = {
                let mut player = TestPlayer::try_new().expect("test player construction failed");
                player.load_movie(PROBE_MOVIE).await;
                player.init_movie_at(0).await;
                let initial = native_clock_values(&player);
                assert_eq!(initial, (StaticDatum::Int(0), StaticDatum::Int(0)));
                assert_eq!(
                    matches!(&initial.0, StaticDatum::Int(ticks) if (*ticks as f64 / 60.0) > 0.0),
                    false,
                    "the source ambient predicate must be closed at native t=0"
                );
                player.advance_to(100).await;
                let at_100 = native_clock_values(&player);
                (
                    at_100.0.clone(),
                    at_100.1.clone(),
                    StaticDatum::Int(if matches!(&at_100.0, StaticDatum::Int(ticks) if (*ticks as f64 / 60.0) > 0.0) { 1 } else { 0 }),
                )
            };

            let split = {
                let mut player = TestPlayer::try_new().expect("test player construction failed");
                player.load_movie(PROBE_MOVIE).await;
                player.init_movie_at(0).await;
                player.advance_to(50).await;
                player.advance_to(100).await;
                let at_100 = native_clock_values(&player);
                (
                    at_100.0.clone(),
                    at_100.1.clone(),
                    StaticDatum::Int(if matches!(&at_100.0, StaticDatum::Int(ticks) if (*ticks as f64 / 60.0) > 0.0) { 1 } else { 0 }),
                )
            };

            assert_eq!(combined, (StaticDatum::Int(6), StaticDatum::Int(100), StaticDatum::Int(1)));
            assert_eq!(split, combined, "split and combined native clocks must agree");
        });
    }

    #[test]
    fn native_invoke_uses_owner_command_queue_for_sync_async_and_stale_paths() {
        run_test(async {
            let mut player = TestPlayer::try_new().expect("test player construction failed");
            player.load_movie(PROBE_MOVIE).await;
            player.init_movie_at(0).await;

            let sync = player
                .native_invoke_global_quiet(
                    "integerp",
                    vec![NativeInvokeArgument::Int(1)],
                )
                .await
                .expect("synchronous global should return through the owner queue");
            assert_eq!(sync, NativeGlobalValue::Int(1));

            let async_result = player
                .native_invoke_global_quiet("go", vec![NativeInvokeArgument::Int(1)])
                .await
                .expect("owned asynchronous global should complete through the command pump");
            assert!(matches!(async_result, NativeGlobalValue::Void));

            let error = player
                .native_invoke_global_quiet("native_missing_handler", vec![])
                .await
                .expect_err("unknown handler must return a structured error");
            assert!(matches!(error, NativeInvokeError::Runtime(error) if error.message.contains("handler") || error.message.contains("Handler")));

            player.runtime.retire_current();
            let stale = player
                .native_invoke_global_quiet("value", vec![NativeInvokeArgument::Void])
                .await
                .expect_err("retired owner must reject invocation before queueing");
            assert!(matches!(stale, NativeInvokeError::Runtime(error) if error.message.contains("stale")));
        });
    }

    #[test]
    fn native_object_stop_apply_opcode_continuation_completes_once() {
        run_test(async {
            let mut player = TestPlayer::try_new().expect("test player construction failed");
            player.load_movie(PROBE_MOVIE).await;
            player.init_movie_at(0).await;

            // This is the source-shaped form emitted by SndSFX: the evaluator
            // classifies `sound(1).stop()` as an owner-bound Object request
            // with an InternalInvocation/ApplyOpcode ticket. The command loop
            // must admit, execute, and resume that exact request once.
            let first = player
                .eval_datum_quiet("sound(1).stop()")
                .await
                .expect("Object/stop continuation must complete through the owner pump");
            let second = player
                .eval_datum_quiet("sound(1).stop()")
                .await
                .expect("repeating Object/stop must not retain a stale action");
            assert_eq!(first, StaticDatum::Void);
            assert_eq!(second, StaticDatum::Void);
            assert_eq!(
                player
                    .runtime
                    .session()
                    .borrow()
                    .has_pending_commands(player.runtime.player_id()),
                false,
                "Object/stop must retire its ApplyOpcode continuation exactly once"
            );
        });
    }

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

    #[test]
    fn native_test_player_sink_accepts_over_capacity_command_notifications() {
        run_test(async {
            let mut player = TestPlayer::new();
            player.load_movie(PROBE_MOVIE).await;
            player.init_movie_at(0).await;
            player
                .eval_datum("value(\"frameState\")")
                .await
                .expect("initial command must complete before recording");
            player.native_notification_observer.recorded.borrow_mut().clear();
            player.native_notification_observer.delivered.set(0);
            player.native_notification_observer.last_owner.set(None);
            let owner = player.runtime.owner().clone();
            player
                .runtime
                .with_context(|context| {
                    for frame in 0..=256 {
                        context.player.queue_player_notification(
                            crate::player::cast_lib::PlayerNotificationKind::Host(
                                crate::player::host_events::HostEvent::FrameChanged { frame },
                            ),
                        );
                    }
                })
                .expect("test player must remain live");

            player
                .runtime
                .dispatch(PlayerVMCommand::DrainInputFlagCleanup)
                .await
                .expect("command-loop command must accept the batch");

            let events = player.native_notification_observer.recorded.borrow();
            assert!(events.len() >= 257);
            assert_eq!(player.native_notification_observer.delivered.get(), events.len() as u64);
            assert_eq!(
                player.native_notification_observer.last_owner.get(),
                Some(owner.key())
            );
            assert!(
                player
                    .runtime
                    .with_context(|context| context.player.host_event_backpressure.is_none())
                    .unwrap()
            );
            let expected_frames: Vec<_> = (0..=256).collect();
            let observed_frames: Vec<_> = events
                .iter()
                .filter_map(|event| match &event.kind {
                    crate::player::host_events::NativePlayerNotificationKind::Host(
                        crate::player::host_events::HostEvent::FrameChanged { frame },
                    ) => Some(*frame),
                    _ => None,
                })
                .collect();
            assert!(observed_frames
                .windows(expected_frames.len())
                .any(|window| window == expected_frames.as_slice()));
            for event in events.iter().filter(|event| {
                matches!(
                    event.kind,
                    crate::player::host_events::NativePlayerNotificationKind::Host(
                        crate::player::host_events::HostEvent::FrameChanged { .. }
                    )
                )
            }) {
                assert!(event.owner.same_identity(&owner));
            }
            assert!(
                player
                    .runtime
                    .with_context(|context| context.player.owner.same_identity(&owner))
                    .unwrap()
            );
            player
                .eval_datum("value(\"frameState\")")
                .await
                .expect("subsequent command must remain usable");
        });
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
        std::fs::write(
            evidence_dir.join(format!("{stem}-before.rgba")),
            &record.before_rgba,
        )
        .unwrap();
        std::fs::write(
            evidence_dir.join(format!("{stem}-input.rgba")),
            &record.input_rgba,
        )
        .unwrap();
        std::fs::write(
            evidence_dir.join(format!("{stem}-final.rgba")),
            &record.final_rgba,
        )
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
        assert_eq!(
            player.try_advance_to(0).await.unwrap(),
            NativeAdvanceReport::default()
        );

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
        assert_eq!(
            at_100,
            NativeAdvanceReport {
                frames: 3,
                timeouts: 2
            }
        );
        assert_eq!(at_100_frame, 1);
        assert_eq!(at_100_frame_state, StaticDatum::Int(141));
        assert!(player.take_native_timeout_host_unsupported().is_empty());

        let at_300 = player.advance_to(300).await;
        let at_300_frame = player.current_frame();
        let at_300_frame_state = player.eval_datum("value(\"frameState\")").await.unwrap();
        assert_eq!(
            at_300,
            NativeAdvanceReport {
                frames: 6,
                timeouts: 1
            }
        );
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
        assert!(
            session
                .borrow_mut()
                .with_player(player_id, |_| ())
                .is_none()
        );
        assert!(session.borrow_mut().take_timeout_host_actions().is_empty());
        assert!(
            session
                .borrow_mut()
                .take_native_timeout_host_unsupported()
                .is_empty()
        );
        assert!(
            async_std::future::timeout(Duration::from_secs(1), command_future)
                .await
                .is_ok(),
            "retired command completer hung"
        );
        assert!(stale_frame_pump.advance_to(301).await.is_err());
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
