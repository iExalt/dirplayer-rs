//! Owner-bound native Flash host backed by the pinned native Ruffle API.
//!
//! Native Flash work is deliberately kept outside the Director session borrow:
//! Ruffle owns its own VM and offscreen WGPU renderer, while the short apply
//! phase copies an accepted RGBA frame into the Director bitmap manager.

use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

use ruffle_core::{
    FloatDuration, LoadBehavior, Player, PlayerBuilder, ViewportDimensions,
    tag_utils::SwfMovie,
};
use ruffle_render_wgpu::{backend::WgpuRenderBackend, target::TextureTarget, wgpu};

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
    width: u32,
    height: u32,
    player: NativeRufflePlayer,
}

/// One native Ruffle instance map per RuntimeSession player. The host has no
/// session handle of its own, preventing a strong cycle; the session stores a
/// weak binding and supplies the handle only for the short frame-apply phase.
pub(crate) struct NativeFlashHost {
    instances: HashMap<i16, NativeFlashInstance>,
}

impl NativeFlashHost {
    pub(crate) fn new() -> Self {
        Self { instances: HashMap::new() }
    }

    pub(crate) fn clear(&mut self) {
        self.instances.clear();
    }

    fn build_player(data: &[u8], width: u32, height: u32) -> Result<NativeRufflePlayer, ScriptError> {
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
            .build();
        {
            let mut player_guard = player
                .lock()
                .map_err(|_| ScriptError::new("native Ruffle player lock is poisoned".to_owned()))?;
            // These controls must precede root installation and the first
            // frame, so initial ActionScript observes the qualified state.
            player_guard.set_parity_seed(0);
            player_guard.set_parity_time_us(0);
            player_guard.set_parity_mode(true);
            player_guard.update(|context| context.set_root_movie(movie));
            // The barrier executes the first root frame and submits a complete
            // render before the frame is exposed to Director.
            player_guard.run_frame();
            player_guard.render();
        }
        Ok(player)
    }

    fn capture(instance: &mut NativeFlashInstance) -> Result<Vec<u8>, ScriptError> {
        let mut player = instance
            .player
            .lock()
            .map_err(|_| ScriptError::new("native Ruffle player lock is poisoned".to_owned()))?;
        player.render();
        let backend = (&mut *player.renderer_mut() as &mut dyn std::any::Any)
            .downcast_mut::<WgpuRenderBackend<TextureTarget>>()
            .ok_or_else(|| ScriptError::new("native Ruffle renderer type changed".to_owned()))?;
        let image = backend
            .capture_frame()
            .ok_or_else(|| ScriptError::new("native Ruffle frame is not complete".to_owned()))?;
        Ok(image.into_raw())
    }

    fn apply_frame(
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
        data: &[u8],
        width: u32,
        height: u32,
        _paused_at_start: bool,
        _asserted_frame: i32,
    ) -> Result<(), ScriptError> {
        if !owner.is_arena_live() {
            return Err(ScriptError::new("native Flash load owner is stale".to_owned()));
        }
        // A newer-generation Load is the replacement operation. Retire the
        // prior Ruffle instance before constructing the replacement.
        self.instances.remove(&sprite);
        let _clock = ControlledTimeGuard::new(0)?;
        let player = Self::build_player(data, width, height)?;
        let mut instance = NativeFlashInstance {
            generation,
            width: width.max(1),
            height: height.max(1),
            player,
        };
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
        let _clock = ControlledTimeGuard::new(0)?;
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
