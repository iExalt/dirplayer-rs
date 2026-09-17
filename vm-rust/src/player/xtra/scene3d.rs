// SPDX-License-Identifier: GPL-3.0-only
//
//! Host-side store for the external Xtra 3D scene API (`HostOp::Scene3d*`).
//!
//! An external plugin (e.g. a Groove-Xtra wasm) owns all world simulation and,
//! per step, pushes *retained* draw data at the host through the `scene3d_*`
//! host services. Those calls arrive mid-Lingo-execution, with no GL context in
//! scope, so they cannot touch WebGL directly. This module is the buffer
//! between them and the renderer: the [`host_call_dispatch`] arms in
//! `external.rs` write here, and [`XtraSceneRenderer`] in
//! `rendering_gpu::webgl2::xtra_scene` reads here during the normal draw pass,
//! uploading GL buffers/textures lazily and compositing the latest frame.
//!
//! This mirrors the built-in Groove split (`GrooveXtraManager` state ←
//! `GrooveSceneRenderer`), but is engine-agnostic: any plugin driving the
//! `scene3d` ops shares it.

use std::collections::HashMap;

use xtra_sdk::scene3d::{FrameData, MeshData};

/// One uploaded mesh (the batch set for a shape or a deformed object). `generation`
/// bumps on every re-upload so the renderer knows to rebuild its GL buffers.
pub struct SceneMesh {
    pub data: MeshData,
    pub generation: u64,
}

/// One CPU-composed texture uploaded by the plugin (`Scene3dUploadTexture`).
/// Cast-member textures are *not* stored here — the renderer resolves those by
/// name against movie bitmaps. `generation` bumps on re-upload.
pub struct UploadedTexture {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
    pub generation: u64,
}

/// One scene: its meshes, plugin-uploaded textures, and the latest frame to
/// composite, plus the staleness bookkeeping the renderer uses to stop
/// painting once the game stops stepping.
pub struct Scene {
    pub meshes: HashMap<u32, SceneMesh>,
    pub textures: HashMap<String, UploadedTexture>,
    pub frame: Option<FrameData>,
    /// The movie frame in effect when the latest frame was submitted. The
    /// renderer only composites while this equals the current movie frame, so
    /// stale 3D stops painting over later 2D/Flash content (mirrors the
    /// Groove `last_active_frame` gate).
    pub last_submit_frame: i32,
    /// Draws since the last submit; the renderer bumps this and stops after a
    /// small tolerance so a redraw not paired with a step doesn't linger.
    pub draws_since_submit: i32,
}

impl Scene {
    fn new() -> Self {
        Scene {
            meshes: HashMap::new(),
            textures: HashMap::new(),
            frame: None,
            last_submit_frame: -1,
            draws_since_submit: 0,
        }
    }
}

/// The whole store: tag→id map (for idempotent `create`) plus the live scenes.
#[derive(Default)]
pub struct Scene3dStore {
    tags: HashMap<String, i32>,
    pub scenes: HashMap<i32, Scene>,
    next_id: i32,
}

impl Scene3dStore {
    pub fn new() -> Self {
        Scene3dStore { tags: HashMap::new(), scenes: HashMap::new(), next_id: 1 }
    }

    /// Idempotent per tag: returns the existing scene id or mints a new one.
    pub fn create(&mut self, tag: &str) -> i32 {
        if let Some(id) = self.tags.get(tag) {
            return *id;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.tags.insert(tag.to_string(), id);
        self.scenes.insert(id, Scene::new());
        id
    }

    pub fn upload_mesh(&mut self, scene_id: i32, mesh_id: u32, data: MeshData) {
        if let Some(scene) = self.scenes.get_mut(&scene_id) {
            let generation = scene.meshes.get(&mesh_id).map(|m| m.generation + 1).unwrap_or(0);
            scene.meshes.insert(mesh_id, SceneMesh { data, generation });
        }
    }

    pub fn drop_mesh(&mut self, scene_id: i32, mesh_id: u32) {
        if let Some(scene) = self.scenes.get_mut(&scene_id) {
            scene.meshes.remove(&mesh_id);
        }
    }

    pub fn upload_texture(&mut self, scene_id: i32, name: &str, w: u32, h: u32, rgba: Vec<u8>) {
        if let Some(scene) = self.scenes.get_mut(&scene_id) {
            let generation = scene.textures.get(name).map(|t| t.generation + 1).unwrap_or(0);
            scene.textures.insert(name.to_string(), UploadedTexture { w, h, rgba, generation });
        }
    }

    /// Store the latest frame and stamp the movie frame it was submitted on.
    pub fn submit_frame(&mut self, scene_id: i32, frame: FrameData, movie_frame: i32) {
        if let Some(scene) = self.scenes.get_mut(&scene_id) {
            scene.frame = Some(frame);
            scene.last_submit_frame = movie_frame;
            scene.draws_since_submit = 0;
        }
    }

    pub fn destroy(&mut self, scene_id: i32) {
        self.scenes.remove(&scene_id);
        self.tags.retain(|_, id| *id != scene_id);
    }

    /// Remove all scenes owned by this player while retaining the id sequence.
    /// Keeping ids monotonic prevents a reset player from reusing stale
    /// renderer cache keys for the same WebGL renderer.
    pub fn reset(&mut self) {
        self.scenes.clear();
        self.tags.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::Scene3dStore;
    #[cfg(not(target_arch = "wasm32"))]
    use crate::player::ownership::{OwnerKey, OwnerToken};
    #[cfg(not(target_arch = "wasm32"))]
    use crate::player::DirPlayer;
    #[cfg(not(target_arch = "wasm32"))]
    use async_std::channel;

    #[test]
    fn stores_isolate_overlapping_tags_and_reset() {
        let mut first = Scene3dStore::new();
        let mut second = Scene3dStore::new();

        let first_scene = first.create("shared-tag");
        let second_scene = second.create("shared-tag");
        assert_eq!(first_scene, 1);
        assert_eq!(second_scene, 1);
        assert!(first.scenes.contains_key(&first_scene));
        assert!(second.scenes.contains_key(&second_scene));

        first.reset();
        assert!(first.scenes.is_empty());
        assert!(second.scenes.contains_key(&second_scene));

        // The reset player must not reuse a retired renderer cache key.
        let replacement_scene = first.create("shared-tag");
        assert_eq!(replacement_scene, 2);
        assert!(second.scenes.contains_key(&second_scene));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn player_reset_clears_only_its_scene_store() {
        let (first_tx, _) = channel::unbounded();
        let (second_tx, _) = channel::unbounded();
        let mut first = DirPlayer::new_with_owner(
            first_tx,
            OwnerToken::new(OwnerKey { session: 11, player: 1, generation: 1 }),
        );
        let mut second = DirPlayer::new_with_owner(
            second_tx,
            OwnerToken::new(OwnerKey { session: 11, player: 2, generation: 1 }),
        );

        let first_scene = first.scene3d_store.create("shared-tag");
        let second_scene = second.scene3d_store.create("shared-tag");
        assert_eq!(first_scene, second_scene);
        assert!(first.scene3d_store.scenes.contains_key(&first_scene));
        assert!(second.scene3d_store.scenes.contains_key(&second_scene));

        first.reset_owned_core();
        assert!(first.scene3d_store.scenes.is_empty());
        assert!(second.scene3d_store.scenes.contains_key(&second_scene));
    }
}
