//! One bounded Bevy-host scheduling probe for the native DirPlayer path.
//!
//! Bevy owns the probe resource, phase transitions, fixed update count,
//! observation, and retirement. Director and embedded Ruffle remain behind the
//! existing `TestPlayer` and `NativeFramePump` path.

use std::{
    cell::RefCell,
    collections::BTreeMap,
    env,
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
    rc::Rc,
    time::Instant,
};

use bevy::prelude::*;
use bevy_state::app::StatesPlugin;
use serde_json::json;
use sha2::{Digest, Sha256};
use url::Url;

use crate::player::session::NativeFlashCallbackObservation;
use crate::{
    native_parity_worker::QualifiedExternalCasts,
    player::{
        NativeFlashActionSummary, NativeFlashBindingObservation,
        testing::{NativeTeardownWitness, StageSnapshot, TestPlayer},
    },
    rendering::NativePresentationViewport,
};

const ENDPOINT_US: u64 = 2_450_000;
const STEP_US: u64 = 50_000;
const STEP_COUNT: u32 = 49;
const DCR_SHA256: &str = "ddf24b667a8d014856d9db42e1658cbf9714847f1cadc2c2c14d21f8e5950c77";
const CAST_SHA256: &str = "bd18e2325e2074a24b86db5a921b873e1f55bd38db7a7646a57eb0d929006035";
const CAST_FILES: [(&str, &str); 5] = [
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

#[derive(Clone, Debug)]
struct ProbeConfig {
    resource_root: PathBuf,
    movie: PathBuf,
    source_hash: String,
    output_root: PathBuf,
}

impl ProbeConfig {
    fn from_env() -> Result<Self, String> {
        let resource_root = PathBuf::from(env::var("SPYBOT_RESOURCE_ROOT").map_err(|_| {
            "SPYBOT_RESOURCE_ROOT must point to the original Spybot resource root".to_owned()
        })?);
        let movie_name =
            env::var("SPYBOT_MOVIE").unwrap_or_else(|_| "spybot-nightfall-incident.dcr".to_owned());
        let movie = resource_root.join(movie_name);
        let source_hash = env::var("SPYBOT_DCR_SHA256").unwrap_or_else(|_| DCR_SHA256.to_owned());
        let output_root = env::var("SPYBOT_PROBE_OUTPUT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(".cache/native-bevy-schedule-probe"));
        Ok(Self {
            resource_root,
            movie,
            source_hash,
            output_root,
        })
    }

    fn verify_inputs(&self) -> Result<(Vec<u8>, QualifiedExternalCasts), String> {
        let movie_bytes = fs::read(&self.movie)
            .map_err(|error| format!("cannot read {}: {error}", self.movie.display()))?;
        let actual_hash = sha256(&movie_bytes);
        if actual_hash != self.source_hash {
            return Err(format!(
                "source DCR hash mismatch: expected {}, actual {}",
                self.source_hash, actual_hash
            ));
        }
        let movie_dir = self
            .movie
            .parent()
            .ok_or_else(|| "source movie has no parent directory".to_owned())?;
        let movie_base = Url::from_directory_path(movie_dir)
            .map_err(|_| "source movie directory cannot form a file URL".to_owned())?;
        let mut entries = BTreeMap::new();
        for (name, file_name) in CAST_FILES {
            let path = self.resource_root.join(file_name);
            let bytes = fs::read(&path).map_err(|error| {
                format!("cannot read qualified cast {}: {error}", path.display())
            })?;
            let actual_cast_hash = sha256(&bytes);
            if actual_cast_hash != CAST_SHA256 {
                return Err(format!(
                    "qualified cast {name} hash mismatch: expected {CAST_SHA256}, actual {actual_cast_hash}"
                ));
            }
            let requested_url = movie_base
                .join(name)
                .map_err(|_| format!("cannot form requested URL for {name}"))?
                .to_string();
            entries.insert(name.to_owned(), (requested_url, bytes));
        }
        Ok((
            movie_bytes,
            QualifiedExternalCasts::from_entries(
                entries
                    .into_iter()
                    .map(|(key, (url, bytes))| (key, url, bytes)),
            ),
        ))
    }

    fn movie_string(&self) -> Result<String, String> {
        self.movie
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| "source movie path is not valid UTF-8".to_owned())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CaptureEvidence {
    width: u32,
    height: u32,
    sha256: String,
    data: Vec<u8>,
}

#[derive(Clone, Debug)]
struct RunEvidence {
    t0: CaptureEvidence,
    endpoint: CaptureEvidence,
    director_frame: u32,
    director_label: Option<String>,
    initial_bindings: Vec<NativeFlashBindingObservation>,
    endpoint_flash_frames: Vec<u32>,
    endpoint_pending_actions: Vec<NativeFlashActionSummary>,
    callbacks: Vec<NativeFlashCallbackObservation>,
    teardown_ok: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, States)]
enum ProbePhase {
    #[default]
    Initialize,
    CaptureT0,
    Advance,
    CaptureEndpoint,
    Teardown,
    Complete,
}

struct ProbeResource {
    config: ProbeConfig,
    started: Instant,
    control: RunEvidence,
    player: Option<TestPlayer>,
    witness: Option<NativeTeardownWitness>,
    callbacks: Rc<RefCell<Vec<NativeFlashCallbackObservation>>>,
    t0: Option<CaptureEvidence>,
    initial_bindings: Option<Vec<NativeFlashBindingObservation>>,
    steps: u32,
    probe: Option<RunEvidence>,
    failure: Option<String>,
}

fn phase_log(started: &Instant, phase: &str) {
    let elapsed_ms = started.elapsed().as_millis();
    let mut stdout = io::stdout().lock();
    let _ = writeln!(stdout, "probe-phase phase={phase} elapsed_ms={elapsed_ms}");
    let _ = stdout.flush();
}

impl ProbeResource {
    fn fail(&mut self, message: impl Into<String>) {
        if self.failure.is_none() {
            self.failure = Some(message.into());
        }
    }
}

fn initialize_system(
    mut probe: NonSendMut<ProbeResource>,
    mut next: ResMut<NextState<ProbePhase>>,
) {
    phase_log(&probe.started, "bevy.initialize.begin");
    let result = (|| {
        phase_log(&probe.started, "bevy.initialize.input_verification.begin");
        let (movie_bytes, qualified_casts) = probe.config.verify_inputs()?;
        phase_log(&probe.started, "bevy.initialize.input_verification.done");
        let mut player = TestPlayer::try_new().map_err(|error| error.message)?;
        phase_log(&probe.started, "bevy.initialize.player_construction.done");
        player
            .set_deterministic_seed(0)
            .map_err(|error| error.message)?;
        let movie = probe.config.movie_string()?;
        async_std::task::block_on(player.load_movie_quiet_with_qualified_casts(
            &movie,
            movie_bytes,
            &qualified_casts,
        ))
        .map_err(|error| format!("native movie load failed: {error:?}"))?;
        phase_log(&probe.started, "bevy.initialize.movie_load.done");
        player
            .configure_native_presentation(Some(NativePresentationViewport {
                x: 0,
                y: 0,
                width: 650,
                height: 420,
            }))
            .map_err(|error| error.message)?;
        phase_log(&probe.started, "bevy.initialize.presentation_config.done");
        player.install_native_flash_callback_observer(probe.callbacks.clone());
        phase_log(
            &probe.started,
            "bevy.initialize.callback_observer_install.done",
        );
        phase_log(&probe.started, "bevy.initialize.movie_init.begin");
        async_std::task::block_on(player.try_init_movie_at_us(0)).map_err(|error| error.message)?;
        phase_log(&probe.started, "bevy.initialize.movie_init.done");
        probe.witness = Some(player.native_teardown_witness());
        probe.player = Some(player);
        Ok::<(), String>(())
    })();
    if let Err(error) = result {
        probe.fail(error);
    } else {
        next.set(ProbePhase::CaptureT0);
    }
}

fn capture_t0_system(
    mut probe: NonSendMut<ProbeResource>,
    mut next: ResMut<NextState<ProbePhase>>,
) {
    phase_log(&probe.started, "bevy.capture_t0.begin");
    let Some(player) = probe.player.as_ref() else {
        probe.fail("probe player missing at t=0 capture");
        return;
    };
    match capture(player) {
        Ok(snapshot) => {
            let initial_bindings = match player.native_init_state_quiet() {
                Ok(state) => state.flash_bindings,
                Err(error) => {
                    probe.fail(error.message);
                    return;
                }
            };
            probe.t0 = Some(snapshot);
            probe.initial_bindings = Some(initial_bindings);
            phase_log(&probe.started, "bevy.capture_t0.done");
            next.set(ProbePhase::Advance);
        }
        Err(error) => probe.fail(error),
    }
}

fn advance_system(mut probe: NonSendMut<ProbeResource>, mut next: ResMut<NextState<ProbePhase>>) {
    let target_us = u64::from(probe.steps + 1) * STEP_US;
    let Some(player) = probe.player.as_mut() else {
        probe.fail("probe player missing during controlled update");
        return;
    };
    if let Err(error) = async_std::task::block_on(player.try_advance_to_us(target_us)) {
        probe.fail(error.message);
        return;
    }
    probe.steps += 1;
    if matches!(probe.steps, 1 | 25 | STEP_COUNT) {
        phase_log(
            &probe.started,
            &format!("bevy.advance.step_{}", probe.steps),
        );
    }
    if probe.steps == STEP_COUNT {
        next.set(ProbePhase::CaptureEndpoint);
    }
}

fn capture_endpoint_system(
    mut probe: NonSendMut<ProbeResource>,
    mut next: ResMut<NextState<ProbePhase>>,
) {
    phase_log(&probe.started, "bevy.capture_endpoint.begin");
    let Some(player) = probe.player.as_ref() else {
        probe.fail("probe player missing at endpoint capture");
        return;
    };
    let Some(t0) = probe.t0.clone() else {
        probe.fail("probe t=0 capture missing");
        return;
    };
    let Some(initial_bindings) = probe.initial_bindings.clone() else {
        probe.fail("probe t=0 source observation missing");
        return;
    };
    match capture(player).and_then(|endpoint| {
        live_evidence(
            player,
            t0,
            endpoint,
            initial_bindings,
            &probe.callbacks.borrow(),
        )
    }) {
        Ok(evidence) => {
            probe.probe = Some(evidence);
            phase_log(&probe.started, "bevy.capture_endpoint.done");
            next.set(ProbePhase::Teardown);
        }
        Err(error) => probe.fail(error),
    }
}

fn teardown_system(mut probe: NonSendMut<ProbeResource>, mut next: ResMut<NextState<ProbePhase>>) {
    phase_log(&probe.started, "bevy.teardown.begin");
    let Some(player) = probe.player.take() else {
        probe.fail("probe player missing at teardown");
        return;
    };
    drop(player);
    let Some(witness) = probe.witness.take() else {
        probe.fail("probe teardown witness missing");
        return;
    };
    let owner_retired = !witness.owner.is_arena_live();
    let native_flash_unbound = witness
        .session
        .borrow()
        .native_flash(witness.player_id)
        .and_then(|binding| binding.upgrade())
        .is_none();
    let native_presentation_unbound = witness
        .session
        .borrow()
        .native_presentation(witness.player_id)
        .and_then(|binding| binding.upgrade())
        .is_none();
    if !(owner_retired && native_flash_unbound && native_presentation_unbound) {
        probe.fail(format!(
            "teardown failed: owner_retired={owner_retired}, native_flash_unbound={native_flash_unbound}, native_presentation_unbound={native_presentation_unbound}"
        ));
        return;
    }
    if let Some(evidence) = probe.probe.as_mut() {
        evidence.teardown_ok = true;
    }
    phase_log(&probe.started, "bevy.teardown.done");
    next.set(ProbePhase::Complete);
}

fn capture(player: &TestPlayer) -> Result<CaptureEvidence, String> {
    let snapshot = player
        .snapshot_rgba_quiet()
        .map_err(|error| error.message)?;
    Ok(capture_evidence(snapshot))
}

fn capture_evidence(snapshot: StageSnapshot) -> CaptureEvidence {
    CaptureEvidence {
        width: snapshot.width,
        height: snapshot.height,
        sha256: sha256(&snapshot.data),
        data: snapshot.data,
    }
}

fn live_evidence(
    player: &TestPlayer,
    t0: CaptureEvidence,
    endpoint: CaptureEvidence,
    initial_bindings: Vec<NativeFlashBindingObservation>,
    callbacks: &[NativeFlashCallbackObservation],
) -> Result<RunEvidence, String> {
    let init = player
        .native_init_state_quiet()
        .map_err(|error| error.message)?;
    let flash = player
        .native_flash_snapshots_quiet()
        .map_err(|error| error.message)?;
    Ok(RunEvidence {
        t0,
        endpoint,
        director_frame: player.current_frame_quiet(),
        director_label: player.current_label_quiet(),
        initial_bindings,
        endpoint_flash_frames: flash
            .into_iter()
            .map(|snapshot| snapshot.current_frame)
            .collect(),
        endpoint_pending_actions: init.pending_flash_actions,
        callbacks: callbacks.to_owned(),
        teardown_ok: false,
    })
}

fn run_native_control(config: &ProbeConfig, started: &Instant) -> Result<RunEvidence, String> {
    phase_log(started, "control.input_verification.begin");
    let (movie_bytes, qualified_casts) = config.verify_inputs()?;
    phase_log(started, "control.input_verification.done");
    let mut player = TestPlayer::try_new().map_err(|error| error.message)?;
    phase_log(started, "control.player_construction.done");
    player
        .set_deterministic_seed(0)
        .map_err(|error| error.message)?;
    let movie = config.movie_string()?;
    async_std::task::block_on(player.load_movie_quiet_with_qualified_casts(
        &movie,
        movie_bytes,
        &qualified_casts,
    ))
    .map_err(|error| format!("native control movie load failed: {error:?}"))?;
    phase_log(started, "control.movie_load.done");
    player
        .configure_native_presentation(Some(NativePresentationViewport {
            x: 0,
            y: 0,
            width: 650,
            height: 420,
        }))
        .map_err(|error| error.message)?;
    phase_log(started, "control.presentation_config.done");
    let callbacks = Rc::new(RefCell::new(Vec::new()));
    player.install_native_flash_callback_observer(callbacks.clone());
    phase_log(started, "control.callback_observer_install.done");
    phase_log(started, "control.movie_init.begin");
    async_std::task::block_on(player.try_init_movie_at_us(0)).map_err(|error| error.message)?;
    phase_log(started, "control.movie_init.done");
    phase_log(started, "control.capture_t0.begin");
    let t0 = capture(&player)?;
    phase_log(started, "control.capture_t0.done");
    let initial_bindings = player
        .native_init_state_quiet()
        .map_err(|error| error.message)?
        .flash_bindings;
    for step in 1..=STEP_COUNT {
        async_std::task::block_on(player.try_advance_to_us(u64::from(step) * STEP_US))
            .map_err(|error| error.message)?;
        if matches!(step, 1 | 25 | STEP_COUNT) {
            phase_log(started, &format!("control.advance.step_{step}"));
        }
    }
    phase_log(started, "control.capture_endpoint.begin");
    let endpoint = capture(&player)?;
    phase_log(started, "control.capture_endpoint.done");
    let mut evidence = live_evidence(&player, t0, endpoint, initial_bindings, &callbacks.borrow())?;
    let witness = player.native_teardown_witness();
    phase_log(started, "control.teardown.begin");
    drop(player);
    evidence.teardown_ok = teardown_ok(witness);
    phase_log(started, "control.teardown.done");
    Ok(evidence)
}

fn teardown_ok(witness: NativeTeardownWitness) -> bool {
    !witness.owner.is_arena_live()
        && witness
            .session
            .borrow()
            .native_flash(witness.player_id)
            .and_then(|binding| binding.upgrade())
            .is_none()
        && witness
            .session
            .borrow()
            .native_presentation(witness.player_id)
            .and_then(|binding| binding.upgrade())
            .is_none()
}

fn compare(control: &RunEvidence, probe: &RunEvidence) -> Result<(), String> {
    for (name, expected, actual) in [
        ("t=0", &control.t0, &probe.t0),
        ("t=2.45s", &control.endpoint, &probe.endpoint),
    ] {
        if expected.width != 650
            || expected.height != 420
            || actual.width != 650
            || actual.height != 420
        {
            return Err(format!("{name} dimensions are not 650x420"));
        }
        if expected.data != actual.data {
            return Err(format!("{name} RGBA bytes differ"));
        }
    }
    if control.director_frame != 6
        || probe.director_frame != 6
        || control.director_label.as_deref() != Some("start")
        || probe.director_label.as_deref() != Some("start")
    {
        return Err("Director endpoint is not frame 6/label start".to_owned());
    }
    if control.endpoint_flash_frames != [419]
        || probe.endpoint_flash_frames != [419]
        || control
            .initial_bindings
            .iter()
            .map(|binding| binding.asserted_frame)
            .collect::<Vec<_>>()
            != [Some(371)]
        || probe
            .initial_bindings
            .iter()
            .map(|binding| binding.asserted_frame)
            .collect::<Vec<_>>()
            != [Some(371)]
    {
        return Err("Flash frame/asserted-frame observations differ from 371->419".to_owned());
    }
    if !control.endpoint_pending_actions.is_empty() || !probe.endpoint_pending_actions.is_empty() {
        return Err("pending Flash actions remain at endpoint".to_owned());
    }
    for (name, run) in [("control", control), ("probe", probe)] {
        if run.callbacks.len() != 1 || run.callbacks[0].url != "lingo:introTitleReady()" {
            return Err(format!(
                "{name} callback observation is not exactly one introTitleReady"
            ));
        }
        if !run.teardown_ok {
            return Err(format!(
                "{name} teardown did not retire and unbind the owner"
            ));
        }
    }
    Ok(())
}

fn write_capture(root: &Path, name: &str, capture: &CaptureEvidence) -> Result<(), String> {
    let mut file = File::create(root.join(name)).map_err(|error| error.to_string())?;
    file.write_all(&capture.data)
        .map_err(|error| error.to_string())
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn write_receipt(
    config: &ProbeConfig,
    control: &RunEvidence,
    probe: &RunEvidence,
) -> Result<(), String> {
    fs::create_dir_all(&config.output_root).map_err(|error| error.to_string())?;
    write_capture(&config.output_root, "control-t0.rgba", &control.t0)?;
    write_capture(
        &config.output_root,
        "control-endpoint.rgba",
        &control.endpoint,
    )?;
    write_capture(&config.output_root, "bevy-t0.rgba", &probe.t0)?;
    write_capture(&config.output_root, "bevy-endpoint.rgba", &probe.endpoint)?;
    let receipt = json!({
        "status": "passed",
        "source": {"movie": config.movie, "sha256": config.source_hash},
        "schedule": {"step_us": STEP_US, "step_count": STEP_COUNT, "endpoint_us": ENDPOINT_US},
        "control": run_json(control),
        "bevy": run_json(probe),
    });
    fs::write(
        config.output_root.join("receipt.json"),
        serde_json::to_vec_pretty(&receipt).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

fn run_json(run: &RunEvidence) -> serde_json::Value {
    json!({
        "t0": {"width": run.t0.width, "height": run.t0.height, "sha256": run.t0.sha256},
        "endpoint": {"width": run.endpoint.width, "height": run.endpoint.height, "sha256": run.endpoint.sha256},
        "director_frame": run.director_frame,
        "director_label": run.director_label,
        "initial_asserted_frames": run.initial_bindings.iter().map(|binding| binding.asserted_frame).collect::<Vec<_>>(),
        "endpoint_flash_frames": run.endpoint_flash_frames,
        "endpoint_pending_actions": run.endpoint_pending_actions.iter().map(|action| json!({"kind": action.kind, "sprite": action.sprite, "generation": action.generation, "cast_lib": action.cast_lib, "cast_member": action.cast_member})).collect::<Vec<_>>(),
        "callbacks": run.callbacks.iter().map(|callback| json!({"now_us": callback.now_us, "url": callback.url, "generation": callback.generation, "sprite": callback.sprite})).collect::<Vec<_>>(),
        "teardown_ok": run.teardown_ok,
    })
}

pub fn run() -> Result<(), String> {
    let config = ProbeConfig::from_env()?;
    let started = Instant::now();
    phase_log(&started, "probe.begin");
    let control = run_native_control(&config, &started)?;
    phase_log(&started, "control.complete");
    let mut app = App::new();
    app.add_plugins(StatesPlugin)
        .init_state::<ProbePhase>()
        .insert_non_send_resource(ProbeResource {
            config: config.clone(),
            started,
            control: control.clone(),
            player: None,
            witness: None,
            callbacks: Rc::new(RefCell::new(Vec::new())),
            t0: None,
            initial_bindings: None,
            steps: 0,
            probe: None,
            failure: None,
        })
        .add_systems(
            Update,
            initialize_system.run_if(in_state(ProbePhase::Initialize)),
        )
        .add_systems(
            Update,
            capture_t0_system.run_if(in_state(ProbePhase::CaptureT0)),
        )
        .add_systems(Update, advance_system.run_if(in_state(ProbePhase::Advance)))
        .add_systems(
            Update,
            capture_endpoint_system.run_if(in_state(ProbePhase::CaptureEndpoint)),
        )
        .add_systems(
            Update,
            teardown_system.run_if(in_state(ProbePhase::Teardown)),
        );
    for _ in 0..(STEP_COUNT as usize + 10) {
        app.update();
        let probe = app.world().non_send_resource::<ProbeResource>();
        if probe.failure.is_some() || probe.probe.as_ref().is_some_and(|run| run.teardown_ok) {
            break;
        }
    }
    let probe = app.world().non_send_resource::<ProbeResource>();
    if let Some(error) = probe.failure.clone() {
        return Err(error);
    }
    let bevy = probe
        .probe
        .clone()
        .ok_or_else(|| "probe did not reach teardown".to_owned())?;
    compare(&probe.control, &bevy)?;
    write_receipt(&probe.config, &probe.control, &bevy)
}
