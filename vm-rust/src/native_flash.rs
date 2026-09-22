//! Owner-bound native Flash host backed by the pinned native Ruffle API.
//!
//! Native Flash work is deliberately kept outside the Director session borrow:
//! Ruffle owns its own VM and offscreen WGPU renderer, while the short apply
//! phase copies an accepted RGBA frame into the Director bitmap manager.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Mutex, OnceLock},
};

use ruffle_core::{
    backend::navigator::{
        ErrorResponse, NavigationMethod, NavigatorBackend, NullNavigatorBackend, OwnedFuture,
        Request, SuccessResponse,
    },
    socket::{SocketAction, SocketHandle},
    events::{MouseButton, PlayerEvent},
    FloatDuration, LoadBehavior, Player, PlayerBuilder, ViewportDimensions,
    tag_utils::SwfMovie,
};
use ruffle_render_wgpu::{backend::WgpuRenderBackend, target::TextureTarget, wgpu};
use async_channel::{Receiver, Sender};
use indexmap::IndexMap;
use url::{ParseError, Url};

use crate::player::{
    ScriptError,
    session::{PlayerId, RuntimeSessionHandle},
};
use crate::player::ownership::OwnerToken;

type NativeRufflePlayer = std::sync::Arc<std::sync::Mutex<Player>>;

/// Ruffle's controlled date is thread-local. Serialize all native Ruffle host
/// work and clear the override before releasing the lock so one owner cannot
/// leak its epoch into another owner on the same thread.
fn controlled_time_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct ControlledTimeGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl ControlledTimeGuard {
    fn new(timestamp_us: i64) -> Result<Self, ScriptError> {
        let lock = controlled_time_lock()
            .lock()
            .map_err(|_| ScriptError::new("native Ruffle time lock is poisoned".to_owned()))?;
        ruffle_core::locale::set_controlled_date_time(Some(timestamp_us))
            .map_err(ScriptError::new)?;
        Ok(Self { _lock: lock })
    }
}

impl Drop for ControlledTimeGuard {
    fn drop(&mut self) {
        let _ = ruffle_core::locale::set_controlled_date_time(None);
    }
}

struct NativeFlashInstance {
    generation: u64,
    cast_lib: i32,
    cast_member: i32,
    width: u32,
    height: u32,
    paused_at_start: bool,
    player: NativeRufflePlayer,
    callbacks: NativeFlashCallbackBufferHandle,
}

const NATIVE_FLASH_CALLBACK_CAPACITY: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NativeFlashCallback {
    pub(crate) sprite: i16,
    pub(crate) generation: u64,
    pub(crate) cast_lib: i32,
    pub(crate) cast_member: i32,
    pub(crate) url: String,
}

#[derive(Default)]
pub(crate) struct NativeFlashCallbackBuffer {
    callbacks: VecDeque<NativeFlashCallback>,
    overflowed: bool,
}

type NativeFlashCallbackBufferHandle = std::sync::Arc<Mutex<NativeFlashCallbackBuffer>>;

struct NativeNavigatorBackend {
    delegate: Mutex<NullNavigatorBackend>,
    callbacks: NativeFlashCallbackBufferHandle,
    sprite: i16,
    generation: u64,
    cast_lib: i32,
    cast_member: i32,
}

impl NativeNavigatorBackend {
    fn new(
        callbacks: NativeFlashCallbackBufferHandle,
        sprite: i16,
        generation: u64,
        cast_lib: i32,
        cast_member: i32,
    ) -> Self {
        Self {
            delegate: Mutex::new(NullNavigatorBackend::new()),
            callbacks,
            sprite,
            generation,
            cast_lib,
            cast_member,
        }
    }
}

impl NavigatorBackend for NativeNavigatorBackend {
    fn navigate_to_url(
        &self,
        url: &str,
        _target: &str,
        _vars_method: Option<(NavigationMethod, IndexMap<String, String>)>,
    ) {
        let accepted = url.starts_with("lingo:") || url.starts_with("event:");
        if !accepted {
            return;
        }
        let Ok(mut callbacks) = self.callbacks.lock() else {
            return;
        };
        if callbacks.callbacks.len() >= NATIVE_FLASH_CALLBACK_CAPACITY {
            callbacks.overflowed = true;
            return;
        }
        callbacks.callbacks.push_back(NativeFlashCallback {
            sprite: self.sprite,
            generation: self.generation,
            cast_lib: self.cast_lib,
            cast_member: self.cast_member,
            url: url.to_owned(),
        });
    }

    fn fetch(&self, request: Request) -> OwnedFuture<Box<dyn SuccessResponse>, ErrorResponse> {
        self.delegate
            .lock()
            .expect("native navigator delegate lock")
            .fetch(request)
    }

    fn resolve_url(&self, url: &str) -> Result<Url, ParseError> {
        self.delegate
            .lock()
            .expect("native navigator delegate lock")
            .resolve_url(url)
    }

    fn spawn_future(&mut self, future: OwnedFuture<(), ruffle_core::loader::Error>) {
        self.delegate
            .lock()
            .expect("native navigator delegate lock")
            .spawn_future(future);
    }

    fn pre_process_url(&self, url: Url) -> Url {
        self.delegate
            .lock()
            .expect("native navigator delegate lock")
            .pre_process_url(url)
    }

    fn connect_socket(
        &mut self,
        host: String,
        port: u16,
        timeout: std::time::Duration,
        handle: SocketHandle,
        receiver: Receiver<Vec<u8>>,
        sender: Sender<SocketAction>,
    ) {
        self.delegate
            .lock()
            .expect("native navigator delegate lock")
            .connect_socket(host, port, timeout, handle, receiver, sender);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NativeFlashSnapshot {
    pub(crate) sprite: i16,
    pub(crate) generation: u64,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) current_frame: u32,
}

pub(crate) struct NativeFlashFrame {
    pub(crate) sprite: i16,
    pub(crate) generation: u64,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rgba: Vec<u8>,
}

pub(crate) struct NativeFlashAdvance {
    pub(crate) frames: Vec<NativeFlashFrame>,
    pub(crate) callback_buffers: Vec<NativeFlashCallbackBufferHandle>,
}

/// One native Ruffle instance map per RuntimeSession player. The host has no
/// session handle of its own, preventing a strong cycle; the session stores a
/// weak binding and supplies the handle only for the short frame-apply phase.
pub(crate) struct NativeFlashHost {
    instances: HashMap<i16, NativeFlashInstance>,
    logical_time_us: u64,
}

impl NativeFlashHost {
    pub(crate) fn new() -> Self {
        Self { instances: HashMap::new(), logical_time_us: 0 }
    }

    pub(crate) fn clear(&mut self) {
        self.instances.clear();
        self.logical_time_us = 0;
    }

    /// Read the native Ruffle playheads without rendering, advancing, or
    /// changing controlled time. The caller validates the RuntimeSession
    /// owner before and after this short host borrow.
    pub(crate) fn snapshots(&self) -> Result<Vec<NativeFlashSnapshot>, ScriptError> {
        let mut snapshots = self
            .instances
            .iter()
            .map(|(&sprite, instance)| {
                let player = instance.player.lock().map_err(|_| {
                    ScriptError::new("native Ruffle player lock is poisoned".to_owned())
                })?;
                Ok(NativeFlashSnapshot {
                    sprite,
                    generation: instance.generation,
                    width: instance.width,
                    height: instance.height,
                    current_frame: player.current_frame().unwrap_or(0) as u32,
                })
            })
            .collect::<Result<Vec<_>, ScriptError>>()?;
        snapshots.sort_by_key(|snapshot| snapshot.sprite);
        Ok(snapshots)
    }

    /// Return the earliest controlled wake at which an active Ruffle instance
    /// can produce another frame or timer callback. Paused Director bindings
    /// are intentionally excluded because `advance_frames` does not tick them.
    pub(crate) fn time_til_next_frame_us(&self) -> Result<Option<u64>, ScriptError> {
        let mut next = None;
        for instance in self.instances.values() {
            if instance.paused_at_start {
                continue;
            }
            let player = instance.player.lock().map_err(|_| {
                ScriptError::new("native Ruffle player lock is poisoned".to_owned())
            })?;
            let micros = u64::try_from(player.time_til_next_frame().as_micros())
                .map_err(|_| ScriptError::new("native Flash wake exceeds supported range".to_owned()))?
                .max(1);
            next = Some(next.map_or(micros, |current: u64| current.min(micros)));
        }
        Ok(next)
    }

    fn target_frame(frame: i32) -> u16 {
        frame.clamp(1, u16::MAX as i32) as u16
    }

    fn mouse_coordinates(
        local_x: i32,
        local_y: i32,
        sprite_w: i32,
        sprite_h: i32,
        native_w: u32,
        native_h: u32,
    ) -> Result<(f64, f64), ScriptError> {
        if sprite_w <= 0 || sprite_h <= 0 || native_w == 0 || native_h == 0 {
            return Err(ScriptError::new(
                "native Flash mouse dimensions must be positive".to_owned(),
            ));
        }
        Ok((
            f64::from(local_x) * f64::from(native_w) / f64::from(sprite_w),
            f64::from(local_y) * f64::from(native_h) / f64::from(sprite_h),
        ))
    }

    fn apply_seek(player: &mut Player, frame: i32, paused_at_start: bool) {
        let target = Self::target_frame(frame);
        if !paused_at_start {
            player.set_is_playing(true);
        }
        player.goto_frame(target, paused_at_start);
    }

    fn build_player(
        data: &[u8],
        width: u32,
        height: u32,
        logical_time_us: u64,
    ) -> Result<NativeRufflePlayer, ScriptError> {
        Self::build_player_with_callbacks(
            data,
            width,
            height,
            logical_time_us,
            std::sync::Arc::new(Mutex::new(NativeFlashCallbackBuffer::default())),
            0,
            0,
            0,
            0,
        )
    }

    fn build_player_with_callbacks(
        data: &[u8],
        width: u32,
        height: u32,
        logical_time_us: u64,
        callbacks: NativeFlashCallbackBufferHandle,
        sprite: i16,
        generation: u64,
        cast_lib: i32,
        cast_member: i32,
    ) -> Result<NativeRufflePlayer, ScriptError> {
        Self::build_player_with_callbacks_inner(
            data,
            width,
            height,
            logical_time_us,
            callbacks,
            sprite,
            generation,
            cast_lib,
            cast_member,
            false,
        )
        .map(|(player, _)| player)
    }

    fn build_player_with_callbacks_and_baseline(
        data: &[u8],
        width: u32,
        height: u32,
        logical_time_us: u64,
        callbacks: NativeFlashCallbackBufferHandle,
        sprite: i16,
        generation: u64,
        cast_lib: i32,
        cast_member: i32,
    ) -> Result<(NativeRufflePlayer, ruffle_core::CheckpointCensus), ScriptError> {
        Self::build_player_with_callbacks_inner(
            data,
            width,
            height,
            logical_time_us,
            callbacks,
            sprite,
            generation,
            cast_lib,
            cast_member,
            true,
        )
        .and_then(|(player, baseline)| {
            baseline
                .map(|baseline| (player, baseline))
                .ok_or_else(|| ScriptError::new("missing no-source Ruffle baseline".to_owned()))
        })
    }

    fn build_player_with_callbacks_inner(
        data: &[u8],
        width: u32,
        height: u32,
        logical_time_us: u64,
        callbacks: NativeFlashCallbackBufferHandle,
        sprite: i16,
        generation: u64,
        cast_lib: i32,
        cast_member: i32,
        capture_baseline: bool,
    ) -> Result<(NativeRufflePlayer, Option<ruffle_core::CheckpointCensus>), ScriptError> {
        let movie = SwfMovie::from_data(
            data,
            "file:///dirplayer-native-embedded.swf".to_owned(),
            None,
        )
        .map_err(|error| ScriptError::new(format!("native Ruffle SWF parse failed: {error}")))?;
        let renderer = WgpuRenderBackend::for_offscreen(
            (width.max(1), height.max(1)),
            wgpu::Backends::PRIMARY,
            wgpu::PowerPreference::HighPerformance,
        )
        .map_err(|error| ScriptError::new(format!("native Ruffle offscreen renderer failed: {error}")))?;
        let player = PlayerBuilder::new()
            .with_renderer(renderer)
            .with_storage(Box::new(
                ruffle_core::backend::storage::MemoryStorageBackend::new(),
            ))
            .with_load_behavior(LoadBehavior::Blocking)
            .with_autoplay(true)
            .with_viewport_dimensions(width.max(1), height.max(1), 1.0)
            .with_navigator(NativeNavigatorBackend::new(
                callbacks,
                sprite,
                generation,
                cast_lib,
                cast_member,
            ))
            .build();
        let baseline;
        {
            let mut player_guard = player
                .lock()
                .map_err(|_| ScriptError::new("native Ruffle player lock is poisoned".to_owned()))?;
            // These controls must precede root installation and the first
            // frame, so initial ActionScript observes the qualified state.
            player_guard.set_parity_seed(0);
            player_guard.set_parity_time_us(logical_time_us);
            player_guard.set_parity_mode(true);
            player_guard.update(|context| context.set_root_movie(movie));
            baseline = capture_baseline.then(|| player_guard.checkpoint_census());
            // The barrier executes the first root frame and submits a complete
            // render before the frame is exposed to Director.
            player_guard.run_frame();
            player_guard.render();
        }
        Ok((player, baseline))
    }

    fn capture_locked(player: &mut Player) -> Result<Vec<u8>, ScriptError> {
        player.render();
        let backend = (&mut *player.renderer_mut() as &mut dyn std::any::Any)
            .downcast_mut::<WgpuRenderBackend<TextureTarget>>()
            .ok_or_else(|| ScriptError::new("native Ruffle renderer type changed".to_owned()))?;
        let image = backend
            .capture_frame()
            .ok_or_else(|| ScriptError::new("native Ruffle frame is not complete".to_owned()))?;
        Ok(image.into_raw())
    }

    fn capture(instance: &mut NativeFlashInstance) -> Result<Vec<u8>, ScriptError> {
        let mut player = instance
            .player
            .lock()
            .map_err(|_| ScriptError::new("native Ruffle player lock is poisoned".to_owned()))?;
        Self::capture_locked(&mut player)
    }

    pub(crate) fn dispatch_mouse(
        &mut self,
        session: &RuntimeSessionHandle,
        player_id: PlayerId,
        owner: &OwnerToken,
        sprite: i16,
        generation: u64,
        event_type: &str,
        local_x: i32,
        local_y: i32,
        sprite_w: i32,
        sprite_h: i32,
    ) -> Result<(), ScriptError> {
        if !owner.is_arena_live() {
            return Err(ScriptError::new("native Flash mouse owner is stale".to_owned()));
        }
        let event = match event_type {
            "move" => PlayerEvent::MouseMove {
                x: 0.0,
                y: 0.0,
            },
            "down" => PlayerEvent::MouseDown {
                x: 0.0,
                y: 0.0,
                button: MouseButton::Left,
                index: None,
            },
            "up" => PlayerEvent::MouseUp {
                x: 0.0,
                y: 0.0,
                button: MouseButton::Left,
            },
            _ => {
                return Err(ScriptError::new(
                    "native Flash mouse event type is unsupported".to_owned(),
                ));
            }
        };
        let (width, height, rgba) = {
            let instance = self.instances.get_mut(&sprite).ok_or_else(|| {
                ScriptError::new("native Flash mouse has no live instance".to_owned())
            })?;
            if instance.generation != generation {
                return Err(ScriptError::new("native Flash mouse generation is stale".to_owned()));
            }
            let owner_current = session
                .borrow_mut()
                .with_player(player_id, |context| {
                    owner.is_arena_live()
                        && owner.same_identity(&context.player.owner)
                        && context
                            .player
                            .is_flash_instance_generation_current(sprite, generation)
                })
                .unwrap_or(false);
            if !owner_current {
                return Err(ScriptError::new(
                    "native Flash mouse owner or generation is stale".to_owned(),
                ));
            }
            let (x, y) = Self::mouse_coordinates(
                local_x,
                local_y,
                sprite_w,
                sprite_h,
                instance.width,
                instance.height,
            )?;
            let event = match event {
                PlayerEvent::MouseMove { .. } => PlayerEvent::MouseMove { x, y },
                PlayerEvent::MouseDown { .. } => PlayerEvent::MouseDown {
                    x,
                    y,
                    button: MouseButton::Left,
                    index: None,
                },
                PlayerEvent::MouseUp { .. } => PlayerEvent::MouseUp {
                    x,
                    y,
                    button: MouseButton::Left,
                },
                _ => unreachable!(),
            };
            let _clock = ControlledTimeGuard::new(
                i64::try_from(self.logical_time_us)
                    .map_err(|_| ScriptError::new("native Flash time exceeds supported range".to_owned()))?,
            )?;
            let mut player = instance
                .player
                .lock()
                .map_err(|_| ScriptError::new("native Ruffle player lock is poisoned".to_owned()))?;
            player.set_parity_time_us(self.logical_time_us);
            let _handled = player.handle_event(event);
            let rgba = Self::capture_locked(&mut player)?;
            (instance.width, instance.height, rgba)
        };
        Self::apply_frame(
            session,
            player_id,
            owner,
            sprite,
            generation,
            width,
            height,
            &rgba,
        )
    }

    fn tick_exact(player: &mut Player, elapsed_us: u64) {
        let mut remaining_us = elapsed_us;
        while remaining_us > 0 {
            let frame_duration_us = if player.frame_rate().is_finite() && player.frame_rate() > 0.0 {
                (1_000_000.0 / player.frame_rate()).ceil().max(1.0) as u64
            } else {
                remaining_us
            };
            let slice_us = remaining_us.min(frame_duration_us).max(1);
            player.tick(FloatDuration::from_millis(slice_us as f64 / 1_000.0));
            remaining_us -= slice_us;
        }
    }

    pub(crate) fn set_time_us(&mut self, logical_time_us: u64) -> Result<(), ScriptError> {
        let _clock = ControlledTimeGuard::new(
            i64::try_from(logical_time_us)
                .map_err(|_| ScriptError::new("native Flash time exceeds supported range".to_owned()))?,
        )?;
        for instance in self.instances.values() {
            let mut player = instance
                .player
                .lock()
                .map_err(|_| ScriptError::new("native Ruffle player lock is poisoned".to_owned()))?;
            player.set_parity_time_us(logical_time_us);
        }
        self.logical_time_us = logical_time_us;
        Ok(())
    }

    pub(crate) fn advance_frames(
        &mut self,
        elapsed_us: u64,
    ) -> Result<NativeFlashAdvance, ScriptError> {
        let _clock = if elapsed_us > 0 {
            Some(ControlledTimeGuard::new(i64::try_from(self.logical_time_us).map_err(
                |_| ScriptError::new("native Flash time exceeds supported range".to_owned()),
            )?)?)
        } else {
            None
        };
        let mut sprites: Vec<i16> = self.instances.keys().copied().collect();
        sprites.sort_unstable();
        let mut frames = Vec::with_capacity(sprites.len());
        for sprite in sprites.iter().copied() {
            let instance = self
                .instances
                .get_mut(&sprite)
                .expect("sorted native Flash sprite disappeared");
            let mut player = instance
                .player
                .lock()
                .map_err(|_| ScriptError::new("native Ruffle player lock is poisoned".to_owned()))?;
            player.set_parity_time_us(self.logical_time_us);
            if !instance.paused_at_start {
                Self::tick_exact(&mut player, elapsed_us);
            }
            let rgba = Self::capture_locked(&mut player)?;
            frames.push(NativeFlashFrame {
                sprite,
                generation: instance.generation,
                width: instance.width,
                height: instance.height,
                rgba,
            });
        }
        let callback_buffers = sprites
            .iter()
            .filter_map(|sprite| self.instances.get(&sprite).map(|instance| instance.callbacks.clone()))
            .collect();
        Ok(NativeFlashAdvance { frames, callback_buffers })
    }

    pub(crate) fn take_callbacks(
        callbacks: &NativeFlashCallbackBufferHandle,
    ) -> Result<Vec<NativeFlashCallback>, ScriptError> {
        let mut callbacks = callbacks
            .lock()
            .map_err(|_| ScriptError::new("native Flash callback lock is poisoned".to_owned()))?;
        if callbacks.overflowed {
            callbacks.overflowed = false;
            callbacks.callbacks.clear();
            return Err(ScriptError::new(
                "native Flash callback buffer overflowed".to_owned(),
            ));
        }
        Ok(callbacks.callbacks.drain(..).collect())
    }

    pub(crate) fn apply_frame(
        session: &RuntimeSessionHandle,
        player_id: PlayerId,
        owner: &OwnerToken,
        sprite: i16,
        generation: u64,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<(), ScriptError> {
        let accepted = session
            .borrow_mut()
            .with_player(player_id, |context| {
                if !owner.is_arena_live()
                    || !owner.same_identity(&context.player.owner)
                    || !context
                        .player
                        .is_flash_instance_generation_current(sprite, generation)
                {
                    return false;
                }
                crate::update_flash_frame_for_player(
                    context.player,
                    context.symbols,
                    sprite as i32,
                    width,
                    height,
                    rgba,
                )
                .is_ok()
            })
            .unwrap_or(false);
        if accepted {
            Ok(())
        } else {
            Err(ScriptError::new(
                "native Flash frame owner or generation is stale".to_owned(),
            ))
        }
    }

    pub(crate) fn load(
        &mut self,
        session: &RuntimeSessionHandle,
        player_id: PlayerId,
        owner: &OwnerToken,
        sprite: i16,
        generation: u64,
        cast_lib: i32,
        cast_member: i32,
        data: &[u8],
        width: u32,
        height: u32,
        paused_at_start: bool,
        asserted_frame: i32,
    ) -> Result<(), ScriptError> {
        if !owner.is_arena_live() {
            return Err(ScriptError::new("native Flash load owner is stale".to_owned()));
        }
        // A newer-generation Load is the replacement operation. Retire the
        // prior Ruffle instance before constructing the replacement.
        self.instances.remove(&sprite);
        let logical_time_us = self.logical_time_us;
        let _clock = ControlledTimeGuard::new(
            i64::try_from(logical_time_us)
                .map_err(|_| ScriptError::new("native Flash time exceeds supported range".to_owned()))?,
        )?;
        let callbacks = std::sync::Arc::new(Mutex::new(NativeFlashCallbackBuffer::default()));
        let player = Self::build_player_with_callbacks(
            data,
            width,
            height,
            logical_time_us,
            callbacks.clone(),
            sprite,
            generation,
            cast_lib,
            cast_member,
        )?;
        let mut instance = NativeFlashInstance {
            generation,
            cast_lib,
            cast_member,
            width: width.max(1),
            height: height.max(1),
            paused_at_start,
            player,
            callbacks,
        };
        if asserted_frame >= 0 {
            let mut player = instance
                .player
                .lock()
                .map_err(|_| ScriptError::new("native Ruffle player lock is poisoned".to_owned()))?;
            player.set_parity_time_us(logical_time_us);
            Self::apply_seek(&mut player, asserted_frame, paused_at_start);
            player.render();
        } else if paused_at_start {
            let mut player = instance
                .player
                .lock()
                .map_err(|_| ScriptError::new("native Ruffle player lock is poisoned".to_owned()))?;
            player.set_parity_time_us(logical_time_us);
            Self::apply_seek(&mut player, 1, true);
            player.render();
        }
        let rgba = Self::capture(&mut instance)?;
        Self::apply_frame(
            session,
            player_id,
            owner,
            sprite,
            generation,
            instance.width,
            instance.height,
            &rgba,
        )?;
        self.instances.insert(sprite, instance);
        Ok(())
    }

    pub(crate) fn seek(
        &mut self,
        session: &RuntimeSessionHandle,
        player_id: PlayerId,
        owner: &OwnerToken,
        sprite: i16,
        generation: u64,
        frame: i32,
    ) -> Result<(), ScriptError> {
        if !owner.is_arena_live() {
            return Err(ScriptError::new("native Flash seek owner is stale".to_owned()));
        }
        let (width, height, rgba) = {
            let _clock = ControlledTimeGuard::new(
                i64::try_from(self.logical_time_us).map_err(|_| {
                    ScriptError::new("native Flash time exceeds supported range".to_owned())
                })?,
            )?;
            let instance = self.instances.get_mut(&sprite).ok_or_else(|| {
                ScriptError::new("native Flash seek has no live instance".to_owned())
            })?;
            if instance.generation != generation {
                return Err(ScriptError::new("native Flash seek generation is stale".to_owned()));
            }
            {
                let mut player = instance
                    .player
                    .lock()
                    .map_err(|_| ScriptError::new("native Ruffle player lock is poisoned".to_owned()))?;
                player.set_parity_time_us(self.logical_time_us);
                Self::apply_seek(&mut player, frame, instance.paused_at_start);
                player.render();
            }
            (instance.width, instance.height, Self::capture(instance)?)
        };
        Self::apply_frame(
            session,
            player_id,
            owner,
            sprite,
            generation,
            width,
            height,
            &rgba,
        )
    }

    pub(crate) fn resize(
        &mut self,
        session: &RuntimeSessionHandle,
        player_id: PlayerId,
        owner: &OwnerToken,
        sprite: i16,
        generation: u64,
        width: u32,
        height: u32,
    ) -> Result<(), ScriptError> {
        let _clock = ControlledTimeGuard::new(
            i64::try_from(self.logical_time_us)
                .map_err(|_| ScriptError::new("native Flash time exceeds supported range".to_owned()))?,
        )?;
        let instance = self.instances.get_mut(&sprite).ok_or_else(|| {
            ScriptError::new("native Flash resize has no live instance".to_owned())
        })?;
        if instance.generation != generation {
            return Err(ScriptError::new("native Flash resize generation is stale".to_owned()));
        }
        instance.width = width.max(1);
        instance.height = height.max(1);
        {
            let mut player = instance
                .player
                .lock()
                .map_err(|_| ScriptError::new("native Ruffle player lock is poisoned".to_owned()))?;
            player.set_parity_time_us(self.logical_time_us);
            player.set_viewport_dimensions(ViewportDimensions {
                width: instance.width,
                height: instance.height,
                scale_factor: 1.0,
            });
            player.render();
        }
        let rgba = Self::capture(instance)?;
        Self::apply_frame(
            session,
            player_id,
            owner,
            sprite,
            generation,
            instance.width,
            instance.height,
            &rgba,
        )
    }

    pub(crate) fn unload(&mut self, sprite: i16, generation: u64) -> Result<(), ScriptError> {
        if self.instances.get(&sprite).is_some_and(|instance| instance.generation == generation) {
            self.instances.remove(&sprite);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{NativeFlashHost, NativeFlashInstance, NativeFlashSnapshot};
    use ruffle_core::avm2::{object::TObject, Value as Avm2Value};
    use flate2::read::ZlibDecoder;
    use ruffle_core::backend::navigator::NavigatorBackend;
    use ruffle_core::FloatDuration;
    use sha2::{Digest, Sha256};
    use std::io::Read;

    fn test_instance(
        player: super::NativeRufflePlayer,
        generation: u64,
        paused_at_start: bool,
    ) -> NativeFlashInstance {
        NativeFlashInstance {
            generation,
            cast_lib: 0,
            cast_member: 0,
            width: 2,
            height: 2,
            paused_at_start,
            player,
            callbacks: std::sync::Arc::new(std::sync::Mutex::new(
                super::NativeFlashCallbackBuffer::default(),
            )),
        }
    }

    fn swf_with_frame_rate(data: &[u8], frame_rate: f64) -> Vec<u8> {
        let mut swf = if data.starts_with(b"CWS") {
            let mut expanded = data[..8].to_vec();
            let mut decoder = ZlibDecoder::new(&data[8..]);
            decoder
                .read_to_end(&mut expanded)
                .expect("test SWF must decompress");
            expanded[0..3].copy_from_slice(b"FWS");
            expanded
        } else {
            data.to_vec()
        };
        let nbits = usize::from(swf[8] >> 3);
        let rect_bytes = (5 + nbits * 4).div_ceil(8);
        let frame_rate_offset = 8 + rect_bytes;
        let fixed_rate = (frame_rate * 256.0).round() as u16;
        swf[frame_rate_offset..frame_rate_offset + 2]
            .copy_from_slice(&fixed_rate.to_le_bytes());
        swf
    }

    #[test]
    fn native_flash_seek_target_is_bounded_to_player_frame_type() {
        assert_eq!(NativeFlashHost::target_frame(-10), 1);
        assert_eq!(NativeFlashHost::target_frame(0), 1);
        assert_eq!(NativeFlashHost::target_frame(1), 1);
        assert_eq!(NativeFlashHost::target_frame(u16::MAX as i32), u16::MAX);
        assert_eq!(NativeFlashHost::target_frame(i32::MAX), u16::MAX);
    }

    #[test]
    fn native_flash_mouse_coordinates_match_browser_scaling() {
        assert_eq!(
            NativeFlashHost::mouse_coordinates(300, 160, 600, 320, 626, 468)
                .expect("positive sprite dimensions"),
            (313.0, 234.0)
        );
        assert!(NativeFlashHost::mouse_coordinates(1, 1, 0, 320, 626, 468).is_err());
        assert!(NativeFlashHost::mouse_coordinates(1, 1, 600, 320, 0, 468).is_err());
    }

    #[test]
    fn native_navigator_buffers_only_owner_qualified_callback_urls() {
        let callbacks = std::sync::Arc::new(std::sync::Mutex::new(
            super::NativeFlashCallbackBuffer::default(),
        ));
        let navigator = super::NativeNavigatorBackend::new(callbacks.clone(), 3, 9, 2, 4);
        navigator.navigate_to_url("https://example.invalid", "_self", None);
        navigator.navigate_to_url("fscommand:allowscale", "", None);
        navigator.navigate_to_url("lingo:introTitleReady()", "", None);
        navigator.navigate_to_url("event:ready 3", "", None);
        let events = NativeFlashHost::take_callbacks(&callbacks).expect("callback buffer drain");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].url, "lingo:introTitleReady()");
        assert_eq!(events[1].url, "event:ready 3");
        assert_eq!(events[0].sprite, 3);
        assert_eq!(events[0].generation, 9);
        assert!(NativeFlashHost::take_callbacks(&callbacks)
            .expect("second callback buffer drain")
            .is_empty());
    }

    #[test]
    fn native_flash_seek_preserves_paused_pin_and_advances_animated_swf() {
        let player = NativeFlashHost::build_player(
            include_bytes!("../../../childhood-redux/resources/battalion-ghosts/battalion-ghosts.swf"),
            2,
            2,
            0,
        )
        .expect("real multi-frame SWF must load");
        let mut player = player.lock().expect("Ruffle player lock");

        NativeFlashHost::apply_seek(&mut player, 2, true);
        assert_eq!(player.current_frame(), Some(2));
        player.tick(FloatDuration::from_millis(10.0));
        assert_eq!(player.current_frame(), Some(2), "paused seek must remain pinned");

        NativeFlashHost::apply_seek(&mut player, 2, false);
        assert_eq!(player.current_frame(), Some(2));
        let mut advanced = false;
        for _ in 0..100 {
            player.tick(FloatDuration::from_millis(10.0));
            if player.current_frame() != Some(2) {
                advanced = true;
                break;
            }
        }
        assert!(advanced, "animated seek must advance on tick");
    }

    #[test]
    fn native_flash_advance_is_ordered_and_zero_delta_is_quiet() {
        let data = include_bytes!("../../../childhood-redux/resources/battalion-ghosts/battalion-ghosts.swf");
        let animated = NativeFlashHost::build_player(data, 2, 2, 0)
            .expect("animated Ruffle player must load");
        let paused = NativeFlashHost::build_player(data, 2, 2, 0)
            .expect("paused Ruffle player must load");
        let mut host = NativeFlashHost::new();
        host.instances.insert(
            7,
            test_instance(paused, 2, true),
        );
        host.instances.insert(
            1,
            test_instance(animated, 1, false),
        );
        host.set_time_us(0).expect("initial native time must apply");
        let before = host.snapshots().expect("initial snapshots must read");
        assert_eq!(before.iter().map(|snapshot| snapshot.sprite).collect::<Vec<_>>(), vec![1, 7]);
        let zero_delta = host
            .advance_frames(0)
            .expect("zero delta must succeed");
        assert_eq!(
            zero_delta
                .frames
                .iter()
                .map(|frame| frame.sprite)
                .collect::<Vec<_>>(),
            vec![1, 7]
        );
        assert_eq!(
            zero_delta
                .frames
                .iter()
                .map(|frame| frame.generation)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(
            zero_delta
                .frames
                .iter()
                .map(|frame| (frame.width, frame.height))
                .collect::<Vec<_>>(),
            vec![(2, 2), (2, 2)]
        );
        assert_eq!(host.snapshots().expect("zero delta snapshots must read"), before);

        host.set_time_us(50_000).expect("controlled native time must apply");
        let frames = host.advance_frames(50_000).expect("native Flash advance must succeed");
        assert_eq!(frames.frames.iter().map(|frame| frame.sprite).collect::<Vec<_>>(), vec![1, 7]);
        let after = host.snapshots().expect("advanced snapshots must read");
        assert_eq!(after[1].current_frame, before[1].current_frame);
        assert_ne!(after[0].current_frame, before[0].current_frame);
    }

    #[test]
    fn native_flash_split_and_combined_advance_match_at_nonintegral_rate() {
        let data = swf_with_frame_rate(
            include_bytes!(
                "../../../childhood-redux/resources/battalion-ghosts/battalion-ghosts.swf"
            ),
            29.97,
        );
        let combined_player = NativeFlashHost::build_player(&data, 2, 2, 0)
            .expect("nonintegral-rate SWF must load");
        let split_player = NativeFlashHost::build_player(&data, 2, 2, 0)
            .expect("nonintegral-rate SWF must load");
        {
            let player = combined_player.lock().expect("combined Ruffle player lock");
            assert!((player.frame_rate() - 29.97).abs() < 0.01);
        }
        let mut combined = NativeFlashHost::new();
        combined.instances.insert(
            1,
            test_instance(combined_player, 1, false),
        );
        let mut split = NativeFlashHost::new();
        split.instances.insert(
            1,
            test_instance(split_player, 1, false),
        );

        combined.set_time_us(100_000).expect("combined time");
        let combined_frames = combined
            .advance_frames(100_000)
            .expect("combined native Flash advance");

        split.set_time_us(50_000).expect("first split time");
        split
            .advance_frames(50_000)
            .expect("first split native Flash advance");
        split.set_time_us(100_000).expect("second split time");
        let split_frames = split
            .advance_frames(50_000)
            .expect("second split native Flash advance");

        assert_eq!(
            combined.snapshots().expect("combined snapshots"),
            split.snapshots().expect("split snapshots")
        );
        assert_eq!(
            combined_frames.frames[0].rgba,
            split_frames.frames[0].rgba,
            "split and combined advancement must render the same frame"
        );
    }

    #[test]
    fn native_flash_opening_anim_seek_371_uses_verified_embedded_swf() {
        let path = std::env::var_os("PARITY_OPENING_ANIM_SWF")
            .expect("PARITY_OPENING_ANIM_SWF must point to the verified opening_anim SWF");
        let bytes = std::fs::read(&path).expect("verified opening_anim SWF must be readable");
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        assert_eq!(
            format!("{:x}", hasher.finalize()),
            "d964a7e594109f8923004333e129f9c655fabde0533a4b4bf74dce238bf3215f",
            "opening_anim bytes must match the recovered embedded SWF"
        );

        let callbacks = std::sync::Arc::new(std::sync::Mutex::new(
            super::NativeFlashCallbackBuffer::default(),
        ));
        let player = NativeFlashHost::build_player_with_callbacks(
            &bytes,
            650,
            420,
            0,
            callbacks.clone(),
            1,
            1,
            1,
            1,
        )
            .expect("verified opening_anim SWF must load in Ruffle");
        let mut instance = NativeFlashInstance {
            generation: 1,
            cast_lib: 0,
            cast_member: 0,
            width: 650,
            height: 420,
            paused_at_start: false,
            player,
            callbacks: std::sync::Arc::new(std::sync::Mutex::new(
                super::NativeFlashCallbackBuffer::default(),
            )),
        };
        {
            let mut player = instance.player.lock().expect("Ruffle player lock");
            NativeFlashHost::apply_seek(&mut player, 371, false);
            assert_eq!(player.current_frame(), Some(371));
            player.render();
        }
        {
            let _clock = super::ControlledTimeGuard::new(2_400_000)
                .expect("controlled frame-419 time");
            let mut player = instance.player.lock().expect("Ruffle player lock");
            NativeFlashHost::tick_exact(&mut player, 2_400_000);
            assert_eq!(player.current_frame(), Some(419));
            player.render();
        }
        let callbacks = NativeFlashHost::take_callbacks(&callbacks)
            .expect("frame-419 callback buffer drain");
        assert_eq!(callbacks.len(), 1);
        assert_eq!(callbacks[0].url, "lingo:introTitleReady()");
        let rgba = NativeFlashHost::capture(&mut instance)
            .expect("opening_anim seek must produce a capturable rendered frame");
        assert_eq!(rgba.len(), 650 * 420 * 4);
    }

    #[test]
    #[ignore = "requires PARITY_OPENING_ANIM_SWF pointing to the verified opening_anim SWF"]
    fn native_flash_checkpoint_census_is_read_only_before_title_continuation() {
        let path = std::env::var_os("PARITY_OPENING_ANIM_SWF")
            .expect("PARITY_OPENING_ANIM_SWF must point to the verified opening_anim SWF");
        let bytes = std::fs::read(&path).expect("verified opening_anim SWF must be readable");
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        assert_eq!(
            format!("{:x}", hasher.finalize()),
            "d964a7e594109f8923004333e129f9c655fabde0533a4b4bf74dce238bf3215f",
            "opening_anim bytes must match the recovered embedded SWF"
        );

        let build_instance = |capture_baseline: bool| {
            let callbacks = std::sync::Arc::new(std::sync::Mutex::new(
                super::NativeFlashCallbackBuffer::default(),
            ));
            let (player, baseline) = if capture_baseline {
                let (player, baseline) = NativeFlashHost::build_player_with_callbacks_and_baseline(
                    &bytes, 650, 420, 0, callbacks.clone(), 1, 1, 1, 1,
                )
                .expect("verified opening_anim SWF must load in Ruffle");
                (player, Some(baseline))
            } else {
                (
                    NativeFlashHost::build_player_with_callbacks(
                        &bytes, 650, 420, 0, callbacks.clone(), 1, 1, 1, 1,
                    )
                    .expect("verified opening_anim SWF must load in Ruffle"),
                    None,
                )
            };
            (
                NativeFlashInstance {
                    generation: 1,
                    cast_lib: 0,
                    cast_member: 0,
                    width: 650,
                    height: 420,
                    paused_at_start: false,
                    player,
                    callbacks: callbacks.clone(),
                },
                callbacks,
                baseline,
            )
        };
        let (mut census_instance, census_callbacks, baseline) = build_instance(true);
        let (mut control_instance, control_callbacks, _) = build_instance(false);
        let baseline = baseline.expect("census build must retain its pre-frame baseline");
        assert!(
            baseline.stage_loader_info.is_some(),
            "baseline must contain Stage LoaderInfo"
        );
        println!(
            "baseline census: phase={} frame={:?} actions={:?} stage_loader_info={:?}",
            baseline.phase,
            baseline.current_frame,
            baseline
                .action_queue
                .iter()
                .map(|action| action.action_type)
                .collect::<Vec<_>>(),
            baseline.stage_loader_info,
        );

        let (before_frame, before_fifo, census) = {
            let mut player = census_instance
                .player
                .lock()
                .expect("Ruffle player lock");
            NativeFlashHost::apply_seek(&mut player, 371, false);
            assert_eq!(player.current_frame(), Some(371));
            let before_frame = player.current_frame();
            let before_fifo = census_callbacks
                .lock()
                .expect("native Flash callback buffer lock")
                .callbacks
                .len();
            let mut census = player.checkpoint_census_with_baseline(Some(&baseline));
            census.host_fifo_depth = Some(before_fifo);
            (before_frame, before_fifo, census)
        };

        assert_eq!(census.roots.len(), 23);
        assert_eq!(census.library_fields.len(), 8);
        let root_field = |name: &str| {
            census
                .roots
                .iter()
                .find(|field| field.name == name)
                .unwrap_or_else(|| panic!("missing census root {name}"))
        };
        assert!(root_field("library").implemented);
        assert_eq!(root_field("library").count, Some(8));
        assert_eq!(root_field("stage").count, Some(1));
        assert_eq!(
            root_field("stage").unsupported,
            None,
            "Stage AVM2 and Stage3D DTOs must match the source-free baseline"
        );
        assert!(census.stage_avm2_object_present);
        assert_eq!(census.stage3d_count, 4);
        println!("stage_avm2_object={:?} stage3d={:?}", census.stage_avm2_object, census.stage3d);
        assert_eq!(root_field("mouse_data").occupancy, ruffle_core::CensusOccupancy::Empty);
        assert_eq!(root_field("interner").count, Some(5120));
        assert!(root_field("interner").implemented);
        assert_eq!(root_field("interner").unsupported, None);
        assert!(
            census
                .avm1_graph
                .string_interner
                .as_ref()
                .is_some_and(|strings| !strings.live_string_values.is_empty()),
            "live weak-string identity census must retain bounded contents"
        );
        for name in [
            "action_queue",
            "load_manager",
            "external_interface",
            "audio_manager",
            "stream_manager",
            "sockets",
            "net_connections",
            "local_connections",
            "orphan_manager",
            "post_frame_callbacks",
        ] {
            let field = root_field(name);
            assert_ne!(field.occupancy, ruffle_core::CensusOccupancy::Unknown, "{name} must have an occupancy classification");
            assert!(field.count.is_some(), "{name} must report a count");
        }
        for name in [
            "load_manager",
            "external_interface",
            "audio_manager",
            "stream_manager",
            "sockets",
            "net_connections",
            "local_connections",
            "orphan_manager",
            "post_frame_callbacks",
        ] {
            let field = root_field(name);
            assert_eq!(field.occupancy, ruffle_core::CensusOccupancy::Empty, "{name} must be empty in the qualified frame-371 slice");
            assert_eq!(field.count, Some(0), "{name} must have zero occupancy");
            assert!(field.implemented, "empty inspected {name} is implemented");
        }
        let dynamic_root = root_field("dynamic_root");
        assert_ne!(dynamic_root.occupancy, ruffle_core::CensusOccupancy::Unknown);
        assert!(dynamic_root.count.is_some());
        if dynamic_root.count.unwrap() == 0 {
            assert_eq!(dynamic_root.occupancy, ruffle_core::CensusOccupancy::Empty);
            assert!(dynamic_root.implemented);
        } else {
            assert_eq!(dynamic_root.occupancy, ruffle_core::CensusOccupancy::Present);
            assert_eq!(dynamic_root.unsupported, Some("dynamic_root_entries_unclassified"));
            assert!(census.unsupported.contains(&"dynamic_root_entries_unclassified"));
        }
        assert!(census.library_fields.iter().all(|field| {
            field.occupancy != ruffle_core::CensusOccupancy::Unknown
                && field.count.is_some()
        }));
        assert_eq!(census.action_queue.len(), 1);
        let action = &census.action_queue[0];
        assert_eq!(action.priority, 1);
        assert_eq!(action.fifo_position, 0);
        assert_eq!(action.action_type, "Construct");
        assert!(!action.is_unload);
        assert_eq!(action.swf_start, None);
        assert_eq!(action.swf_end, None);
        assert_eq!(action.clip_ordinal, Some(5));
        assert!(action.constructor.is_none());
        assert!(action.event_slices.is_empty());
        assert!(action.method_object.is_none());
        assert!(action.method_name.is_none());
        assert!(action.method_args.is_empty());
        assert!(action.notify_listener.is_none());
        assert!(action.notify_method.is_none());
        assert!(action.notify_args.is_empty());
        assert!(action.unsupported.is_none());
        assert_eq!(census.avm2.operand_stack_len, Some(0));
        assert_eq!(census.avm2.execution, ruffle_core::Avm2ExecutionState::Dormant);
        assert_eq!(census.phase, "Idle");
        assert_eq!(census.current_frame, Some(371));
        assert!(census.coverage_complete, "the qualified frame-371 census must satisfy every B2 eligibility gate");
        assert!(census.unsupported.is_empty(), "eligible census must have no unsupported reasons: {:?}", census.unsupported);
        assert!(census.roots.iter().all(|field| field.implemented && field.unsupported.is_none()));
        assert!(census.library_fields.iter().all(|field| field.implemented && field.unsupported.is_none()));
        assert_eq!(census.strong_node_count, Some(census.budget.total_nodes));
        assert_eq!(census.edge_count, Some(census.budget.total_edges));
        assert_eq!(census.weak_edge_count, Some(census.budget.total_weak_entries));
        assert_eq!(census.budget.interner_weak_entries, 5120);
        assert_eq!(census.budget.total_weak_entries, 5120);
        assert!(census.budget.within_budget());
        assert_eq!(census.display_avm1.len(), 11);
        assert_eq!(census.display_avm1[0].display_kind, "Stage");
        assert!(census
            .display_avm1
            .iter()
            .any(|entry| entry.classification == "supported-display-native"));
        assert!(census.avm1_graph.nodes.len() >= census.display_avm1.iter().filter(|entry| entry.has_avm1_object).count());
        assert!(census.avm1_graph.nodes.len() <= 10_000);
        assert!(census.avm1_graph.edges.len() <= 50_000);
        assert_eq!(census.avm1_graph.nodes.len(), 2023);
        assert_eq!(census.avm1_graph.edges.len(), 4199);
        assert_eq!(census.avm1_graph.weak_observations, 0);
        assert!(!census
            .unsupported
            .contains(&"avm1_native_function_registry_identity_unavailable"));
        assert!(census.avm1_graph.nodes.iter().any(|node| {
            node.native_identity
                .as_deref()
                .is_some_and(|identity| identity.starts_with("builtin:"))
        }));
        assert!(census.avm1_graph.nodes.iter().any(|node| node.path.starts_with("root.")));
        assert_eq!(
            census.avm1_graph.stop_reason,
            None
        );
        assert_eq!(census.avm1_graph.stop_path.as_deref(), None);
        let mut native_bindings = census
            .avm1_graph
            .nodes
            .iter()
            .filter_map(|node| match node.function_kind {
                Some("Native") | Some("TableNative") => Some((
                    node.function_kind.unwrap(),
                    node.path.as_str(),
                    node.function_table_index,
                    node.function_constructor_present,
                    node.inbound_alias_count,
                    node.has_display_or_mutable_path,
                    node.native_identity.as_deref(),
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        native_bindings.sort_unstable();
        assert!(!native_bindings.is_empty(), "qualified AVM1 state must expose native function evidence");
        assert!(native_bindings.iter().all(|(_, path, _, _, _, _, identity)| {
            (path.starts_with("root.case_sensitive.") || path.starts_with("root.case_insensitive."))
                && identity.is_some_and(|identity| identity.starts_with("builtin:"))
        }));
        assert!(native_bindings.windows(2).all(|pair| pair[0].1 != pair[1].1), "native canonical paths must be unique");
        let native_count = native_bindings.iter().filter(|binding| binding.0 == "Native").count();
        let table_native_count = native_bindings.iter().filter(|binding| binding.0 == "TableNative").count();
        println!(
            "native_function_summary native_count={native_count} table_native_count={table_native_count} native_representative={:?} table_native_representative={:?} native_target_mismatch_count={} native_identity_blocker_count={} collision_or_blocker_budget_exceeded={}",
            native_bindings.iter().find(|binding| binding.0 == "Native"),
            native_bindings.iter().find(|binding| binding.0 == "TableNative"),
            census.avm1_graph.native_target_mismatch_count,
            census.avm1_graph.native_identity_blockers.len(),
            census.avm1_graph.native_identity_blocker_budget_exceeded,
        );
        assert_eq!(census.avm1_graph.native_target_mismatch_count, 0);
        assert!(census.avm1_graph.native_identity_blockers.is_empty());
        assert!(!census.avm1_graph.native_identity_blocker_budget_exceeded);
        assert!(!census
            .unsupported
            .contains(&"avm1_native_array_property_edges_unclassified"));
        assert_eq!(census.avm2_footprint.operand_stack_len, Some(0));
        assert!(census.avm2_footprint.call_stack_empty);
        assert!(!census.movie_libraries_budget_exceeded);
        assert_eq!(census.movie_libraries.len(), 1);
        let strings = census
            .avm1_graph
            .string_interner
            .as_ref()
            .expect("AVM1 graph census must include string interner evidence");
        assert_eq!(strings.common_atom_count, 394);
        assert_eq!(strings.weak_slot_count, 5120);
        assert_eq!(strings.live_weak_count, 5120);
        assert_eq!(strings.uncovered_live_weak_count, 0);
        let pre_fix_uncovered_by_current_owner_census = vec![
                "_currentframe",
                "_droptarget",
                "_focusrect",
                "_framesloaded",
                "_highquality",
                "_soundbuftime",
                "_totalframes",
                "_xmouse",
                "_xscale",
                "_ymouse",
                "_yscale",
            ];
        println!(
            "pre_fix_uncovered_by_current_owner_census={pre_fix_uncovered_by_current_owner_census:?}"
        );
        assert!(strings.uncovered_by_current_owner_census.is_empty());
        assert!(strings.weak_classification_complete);
        assert!(!census.unsupported.contains(&"live_weak_string_unretained"));
        assert!(census.avm1.root_surfaces.iter().any(|root| {
            root.name == "case_insensitive_system_prototypes"
                && root.count == 32
                && root.classification == "prototype_seed_traversed"
        }));
        assert_eq!(census.avm1.stack_len, 0);
        assert!(census.avm1.register_kinds.iter().all(|kind| {
            *kind == ruffle_core::Avm1ValueKind::Undefined
        }));
        assert_eq!(census.display_traversal.node_count, 11);
        assert!(census.display_traversal.edge_count < census.display_traversal.edge_budget);
        assert!(census.display_traversal.complete);
        assert_eq!(census.display_traversal.stop_reason, None);
        println!("{census}");

        let (mut negative_instance, _, negative_baseline) = build_instance(true);
        let negative_baseline = negative_baseline.expect("negative census baseline");
        let negative = {
            let mut player = negative_instance.player.lock().expect("negative Ruffle player lock");
            NativeFlashHost::apply_seek(&mut player, 371, false);
            let before = player.checkpoint_census_with_baseline(Some(&negative_baseline));
            let before_stage3d = before.stage3d.first().expect("Stage3D before mutation");
            let before_shape = before_stage3d.shape;
            let before_class = before_stage3d.base.class_name.clone();
            let before_vtable_slots = before_stage3d.base.vtable_slot_count;
            let before_property_count = before_stage3d.base.properties.len();
            let before_slot = before_stage3d.base.slots[2].clone();
            player.mutate_with_update_context(|context| {
                let stage3d = context
                    .stage
                    .stage3ds()
                    .iter()
                    .find_map(|object| object.as_stage_3d())
                    .expect("qualified opening_anim has a Stage3D");
                stage3d.set_slot_no_coerce(2, Avm2Value::Number(0.0), context.gc_context);
            });
            let after = player.checkpoint_census_with_baseline(Some(&negative_baseline));
            let after_stage3d = after.stage3d.first().expect("Stage3D after mutation");
            assert_eq!(before_shape, after_stage3d.shape);
            assert_eq!(before_class, after_stage3d.base.class_name);
            assert_eq!(before_vtable_slots, after_stage3d.base.vtable_slot_count);
            assert_eq!(before_property_count, after_stage3d.base.properties.len());
            assert_ne!(before_slot, after_stage3d.base.slots[2]);
            after
        };
        assert!(!negative.coverage_complete);
        assert!(negative.unsupported.contains(&"stage3d_changed"));
        assert!(negative.roots.iter().any(|field| {
            field.name == "stage" && field.unsupported == Some("stage3d_changed")
        }));
        println!("negative_stage3d_slot_mutation: coverage_complete=false blocker=stage3d_changed");

        {
            let mut player = control_instance
                .player
                .lock()
                .expect("Ruffle control player lock");
            NativeFlashHost::apply_seek(&mut player, 371, false);
            assert_eq!(player.current_frame(), Some(371));
        }

        {
            let player = census_instance
                .player
                .lock()
                .expect("Ruffle player lock");
            assert_eq!(player.current_frame(), before_frame);
            let after_fifo = census_callbacks
                .lock()
                .expect("native Flash callback buffer lock")
                .callbacks
                .len();
            assert_eq!(after_fifo, before_fifo);
        }

        let continue_instance = |instance: &mut NativeFlashInstance,
                                 callbacks: &super::NativeFlashCallbackBufferHandle| {
            let _clock = super::ControlledTimeGuard::new(2_400_000)
                .expect("controlled frame-419 time");
            let frame = {
                let mut player = instance.player.lock().expect("Ruffle player lock");
                NativeFlashHost::tick_exact(&mut player, 2_400_000);
                assert_eq!(player.current_frame(), Some(419));
                player.render();
                player.current_frame()
            };
            let callbacks = NativeFlashHost::take_callbacks(callbacks)
                .expect("frame-419 callback buffer drain");
            let rgba = NativeFlashHost::capture(instance)
                .expect("opening_anim continuation must produce a capturable rendered frame");
            (frame, callbacks, rgba)
        };
        let (census_frame, census_callbacks, census_rgba) =
            continue_instance(&mut census_instance, &census_callbacks);
        let (control_frame, control_callbacks, control_rgba) =
            continue_instance(&mut control_instance, &control_callbacks);
        assert_eq!(census_frame, Some(419));
        assert_eq!(control_frame, Some(419));
        assert_eq!(census_callbacks.len(), 1);
        assert_eq!(census_callbacks[0].url, "lingo:introTitleReady()");
        assert_eq!(census_callbacks, control_callbacks);
        assert_eq!(census_rgba.len(), 650 * 420 * 4);
        assert_eq!(control_rgba.len(), 650 * 420 * 4);
        assert_eq!(census_rgba, control_rgba);
        println!(
            "continuation control: callbacks_equal=true rgba_equal=true census_callbacks={:?} control_callbacks={:?} census_rgba_sha256={} control_rgba_sha256={}",
            census_callbacks,
            control_callbacks,
            format!("{:x}", Sha256::digest(&census_rgba)),
            format!("{:x}", Sha256::digest(&control_rgba)),
        );
    }

    #[test]
    fn native_flash_callback_fifo_is_once_for_split_and_combined_advance() {
        let path = std::env::var_os("PARITY_OPENING_ANIM_SWF")
            .expect("PARITY_OPENING_ANIM_SWF must point to the verified opening_anim SWF");
        let bytes = std::fs::read(&path).expect("verified opening_anim SWF must be readable");
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        assert_eq!(
            format!("{:x}", hasher.finalize()),
            "d964a7e594109f8923004333e129f9c655fabde0533a4b4bf74dce238bf3215f",
            "opening_anim bytes must match the recovered embedded SWF"
        );

        let make_host = || {
            let callbacks = std::sync::Arc::new(std::sync::Mutex::new(
                super::NativeFlashCallbackBuffer::default(),
            ));
            let player = NativeFlashHost::build_player_with_callbacks(
                &bytes,
                650,
                420,
                0,
                callbacks.clone(),
                1,
                1,
                1,
                1,
            )
            .expect("verified opening_anim SWF must load in Ruffle");
            let mut host = NativeFlashHost::new();
            host.instances.insert(
                1,
                NativeFlashInstance {
                    generation: 1,
                    cast_lib: 1,
                    cast_member: 1,
                    width: 650,
                    height: 420,
                    paused_at_start: false,
                    player,
                    callbacks: callbacks.clone(),
                },
            );
            {
                let instance = host.instances.get_mut(&1).expect("test Flash instance");
                let mut player = instance.player.lock().expect("Ruffle player lock");
                NativeFlashHost::apply_seek(&mut player, 371, false);
                player.render();
            }
            (host, callbacks)
        };

        let (mut combined, combined_callbacks) = make_host();
        combined
            .set_time_us(2_400_000)
            .expect("combined controlled time");
        combined
            .advance_frames(2_400_000)
            .expect("combined native Flash advance");
        let combined_events = NativeFlashHost::take_callbacks(&combined_callbacks)
            .expect("combined callback drain");
        assert_eq!(combined_events.len(), 1);
        assert_eq!(combined_events[0].url, "lingo:introTitleReady()");
        assert!(NativeFlashHost::take_callbacks(&combined_callbacks)
            .expect("combined empty callback drain")
            .is_empty());

        let (mut split, split_callbacks) = make_host();
        split.set_time_us(2_350_000).expect("split first controlled time");
        split
            .advance_frames(2_350_000)
            .expect("split first native Flash advance");
        assert!(NativeFlashHost::take_callbacks(&split_callbacks)
            .expect("split pre-callback drain")
            .is_empty());
        split.set_time_us(2_400_000).expect("split second controlled time");
        split
            .advance_frames(50_000)
            .expect("split second native Flash advance");
        let split_events = NativeFlashHost::take_callbacks(&split_callbacks)
            .expect("split callback drain");
        assert_eq!(split_events.len(), 1);
        assert_eq!(split_events[0].url, "lingo:introTitleReady()");
        assert!(NativeFlashHost::take_callbacks(&split_callbacks)
            .expect("split empty callback drain")
            .is_empty());
    }

    #[test]
    fn native_flash_snapshot_order_is_deterministic() {
        let mut snapshots = vec![
            NativeFlashSnapshot {
                sprite: 7,
                generation: 2,
                width: 32,
                height: 16,
                current_frame: 3,
            },
            NativeFlashSnapshot {
                sprite: 1,
                generation: 4,
                width: 64,
                height: 24,
                current_frame: 9,
            },
        ];
        snapshots.sort_by_key(|snapshot| snapshot.sprite);
        assert_eq!(snapshots[0].sprite, 1);
        assert_eq!(snapshots[1].sprite, 7);
    }
}
