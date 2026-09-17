//! Scene merging for Director's `loadFile()`.
//!
//! `member(x).loadFile(fileName {, overwrite, generateUniqueNames})`
//! (Director 11.5 Scripting Dictionary) imports the assets of a W3D file into a
//! 3D cast member. With `overwrite = TRUE` (the default) the file simply
//! replaces the member's assets, which needs no merging. This module implements
//! the `overwrite = FALSE` half: splicing a second W3D file's assets into an
//! existing scene, optionally renaming incoming elements that collide.

use crate::player::symbols::symbol::Symbol;
use crate::player::symbols::symbol_table::SymbolTable;
use std::collections::{HashMap, HashSet};

use super::types::W3dScene;

/// Pick a name not already present in `taken` (compared case-insensitively).
///
/// The dictionary says only that a colliding element "is renamed"; it doesn't
/// document the pattern. We reuse the `-clone<N>` suffix this codebase already
/// applies to W3D name collisions in `cloneModelFromCastmember`, so both paths
/// read alike.
fn unique_w3d_name(base: &Symbol, taken: &HashSet<Symbol>, symbols: &mut SymbolTable) -> Result<Symbol, String> {
    if !taken.contains(&base) {
        return Ok(base.clone());
    }
    let base_display = symbols
        .display(base)
        .map_err(|_| "W3D merge encountered a symbol owned by another session".to_owned())?
        .to_owned();
    let mut n = 1;
    loop {
        let candidate = symbols.intern(&format!("{}-clone{}", base_display, n));
        if !taken.contains(&candidate) {
            return Ok(candidate);
        }
        n += 1;
    }
}

/// Apply a rename map (keyed by lowercased original name) to a reference field.
fn remap(field: &mut Symbol, map: &HashMap<Symbol, Symbol>, symbols: &SymbolTable) -> Result<(), String> {
    if field.is_empty() { return Ok(()); }
    symbols.display(field)
        .map_err(|_| "W3D merge encountered a symbol owned by another session".to_owned())?;
    if let Some(new_name) = map.get(field) {
        *field = new_name.clone();
    }
    Ok(())
}

fn validate_symbol(symbol: &Symbol, symbols: &SymbolTable) -> Result<(), String> {
    symbols.display(symbol)
        .map(|_| ())
        .map_err(|_| "W3D merge encountered a symbol owned by another session".to_owned())
}

/// Validate every symbol in both scenes before any merge mutation occurs.
fn validate_scene_symbols(scene: &W3dScene, symbols: &SymbolTable) -> Result<(), String> {
    for material in &scene.materials {
        validate_symbol(&material.name, symbols)?;
    }
    for shader in &scene.shaders {
        validate_symbol(&shader.name, symbols)?;
        validate_symbol(&shader.material_name, symbols)?;
        for layer in &shader.texture_layers {
            validate_symbol(&layer.name, symbols)?;
        }
    }
    for node in &scene.nodes {
        for name in [&node.name, &node.parent_name, &node.resource_name, &node.model_resource_name, &node.shader_name] {
            validate_symbol(name, symbols)?;
        }
    }
    for light in &scene.lights {
        validate_symbol(&light.name, symbols)?;
    }
    for name in scene.texture_images.keys() {
        validate_symbol(name, symbols)?;
    }
    for texture in &scene.texture_infos {
        validate_symbol(&texture.name, symbols)?;
    }
    for skeleton in &scene.skeletons {
        validate_symbol(&skeleton.name, symbols)?;
        for bone in &skeleton.bones {
            validate_symbol(&bone.name, symbols)?;
        }
    }
    for motion in &scene.motions {
        validate_symbol(&motion.name, symbols)?;
        for track in &motion.tracks {
            validate_symbol(&track.bone_name, symbols)?;
        }
    }
    for (key, resource) in &scene.model_resources {
        validate_symbol(key, symbols)?;
        validate_symbol(&resource.name, symbols)?;
        for binding in &resource.shader_bindings {
            validate_symbol(&binding.name, symbols)?;
            for mesh_binding in &binding.mesh_bindings {
                validate_symbol(mesh_binding, symbols)?;
            }
        }
    }
    for (key, meshes) in &scene.clod_meshes {
        validate_symbol(key, symbols)?;
        for mesh in meshes {
            validate_symbol(&mesh.name, symbols)?;
        }
    }
    for key in scene.clod_decoders.keys() {
        validate_symbol(key, symbols)?;
    }
    for mesh in &scene.raw_meshes {
        validate_symbol(&mesh.name, symbols)?;
    }
    Ok(())
}

/// Plan renames for one namespace, recording only the names that actually move.
fn plan_renames(
    names: Vec<Symbol>,
    taken: &mut HashSet<Symbol>,
    out: &mut HashMap<Symbol, Symbol>,
    symbols: &mut SymbolTable,
) -> Result<(), String> {
    // Symbols are interned case-insensitively, so identity already carries the
    // lowercasing this used to do by hand.
    for name in names {
        validate_symbol(&name, symbols)?;
        if name.is_empty() {
            continue;
        }
        let key = name.clone();
        if out.contains_key(&key) || !taken.contains(&key) {
            // Either already planned (the same name appears twice inside the
            // incoming file) or not colliding at all — leave it alone.
            if !taken.contains(&key) {
                taken.insert(key);
            }
            continue;
        }
        let new_name = unique_w3d_name(&name, taken, symbols)?;
        taken.insert(new_name.clone());
        out.insert(key, new_name);
    }
    Ok(())
}

/// Replace a same-named entry or append, comparing names case-insensitively.
fn merge_named<T, F: Fn(&T) -> &Symbol>(
    dst: &mut Vec<T>, src: Vec<T>, name_of: F, symbols: &SymbolTable,
) -> Result<(), String> {
    for item in dst.iter() {
        validate_symbol(name_of(item), symbols)?;
    }
    for item in src {
        let name = name_of(&item);
        validate_symbol(name, symbols)?;
        // All symbols here come from this SymbolTable, whose interned identity
        // is the same case-insensitive comparison Director uses for names.
        match dst.iter().position(|e| name_of(e) == name) {
            Some(i) => dst[i] = item,
            None => dst.push(item),
        }
    }
    Ok(())
}

impl W3dScene {
    /// Merge the assets of `src` into this scene — the `overwrite = FALSE`
    /// behaviour of `loadFile()`.
    ///
    /// `generate_unique_names = true` renames each INCOMING element whose name
    /// collides with one already present, then repoints that file's internal
    /// references at the new names, so neither copy hijacks the other.
    /// `false` lets an incoming element overwrite the same-named existing one,
    /// as the dictionary specifies.
    ///
    /// Names are compared case-insensitively throughout — Director treats W3D
    /// element names that way, and the rest of this module already does.
    pub fn merge_from(
        &mut self,
        mut src: W3dScene,
        generate_unique_names: bool,
        symbols: &mut SymbolTable,
    ) -> Result<(), String> {
        validate_scene_symbols(self, symbols)?;
        validate_scene_symbols(&src, symbols)?;

        // Rename maps are per-namespace: Director lets a shader and a model
        // share a name without colliding, so they must not share a map.
        let mut model_res_renames: HashMap<Symbol, Symbol> = HashMap::new();
        let mut shader_renames: HashMap<Symbol, Symbol> = HashMap::new();
        let mut material_renames: HashMap<Symbol, Symbol> = HashMap::new();
        let mut texture_renames: HashMap<Symbol, Symbol> = HashMap::new();
        let mut node_renames: HashMap<Symbol, Symbol> = HashMap::new();

        if generate_unique_names {
            // Model resources and raw meshes share one namespace: a node's
            // resource_name may refer to either.
            let mut taken_model_res: HashSet<Symbol> = self
                .model_resources
                .keys()
                .cloned()
                .collect();
            taken_model_res.extend(self.raw_meshes.iter().map(|m| m.name.clone()));
            let mut taken_shaders: HashSet<Symbol> =
                self.shaders.iter().map(|s| s.name.clone()).collect();
            let mut taken_materials: HashSet<Symbol> =
                self.materials.iter().map(|m| m.name.clone()).collect();
            // Likewise texture_images (bytes, keyed by name) and texture_infos
            // (metadata) name the same textures.
            let mut taken_textures: HashSet<Symbol> = self
                .texture_images
                .keys()
                .cloned()
                .collect();
            taken_textures.extend(self.texture_infos.iter().map(|t| t.name.clone()));
            let mut taken_nodes: HashSet<Symbol> =
                self.nodes.iter().map(|n| n.name.clone()).collect();

            let mut res_names: Vec<Symbol> = src.model_resources.keys().cloned().collect();
            res_names.extend(src.raw_meshes.iter().map(|m| m.name.clone()));
            plan_renames(res_names, &mut taken_model_res, &mut model_res_renames, symbols)?;
            plan_renames(
                src.shaders.iter().map(|s| s.name.clone()).collect(),
                &mut taken_shaders,
                &mut shader_renames,
                symbols,
            )?;
            plan_renames(
                src.materials.iter().map(|m| m.name.clone()).collect(),
                &mut taken_materials,
                &mut material_renames,
                symbols,
            )?;
            let mut tex_names: Vec<Symbol> = src.texture_images.keys().cloned().collect();
            tex_names.extend(src.texture_infos.iter().map(|t| t.name.clone()));
            plan_renames(tex_names, &mut taken_textures, &mut texture_renames, symbols)?;
            plan_renames(
                src.nodes.iter().map(|n| n.name.clone()).collect(),
                &mut taken_nodes,
                &mut node_renames,
                symbols,
            )?;

            // Rewrite src's own references so the incoming assets stay
            // internally consistent under their new names.
            for shader in &mut src.shaders {
                remap(&mut shader.name, &shader_renames, symbols)?;
                remap(&mut shader.material_name, &material_renames, symbols)?;
                for layer in &mut shader.texture_layers {
                    remap(&mut layer.name, &texture_renames, symbols)?;
                }
            }
            for mat in &mut src.materials {
                remap(&mut mat.name, &material_renames, symbols)?;
            }
            for tex in &mut src.texture_infos {
                remap(&mut tex.name, &texture_renames, symbols)?;
            }
            for node in &mut src.nodes {
                remap(&mut node.name, &node_renames, symbols)?;
                remap(&mut node.parent_name, &node_renames, symbols)?;
                remap(&mut node.shader_name, &shader_renames, symbols)?;
                remap(&mut node.resource_name, &model_res_renames, symbols)?;
                remap(&mut node.model_resource_name, &model_res_renames, symbols)?;
            }
            for mesh in &mut src.raw_meshes {
                remap(&mut mesh.name, &model_res_renames, symbols)?;
            }
            // Keyed collections have to be rebuilt under the new keys.
            src.model_resources = src
                .model_resources
                .into_iter()
                .map(|(k, mut v)| -> Result<_, String> {
                    remap(&mut v.name, &model_res_renames, symbols)?;
                    for binding in &mut v.shader_bindings {
                        for mesh_binding in &mut binding.mesh_bindings {
                            remap(mesh_binding, &shader_renames, symbols)?;
                        }
                    }
                    let mut key = k;
                    remap(&mut key, &model_res_renames, symbols)?;
                    Ok((key, v))
                })
                .collect::<Result<HashMap<_, _>, _>>()?;
            src.clod_meshes = src
                .clod_meshes
                .into_iter()
                .map(|(mut k, v)| -> Result<_, String> {
                    remap(&mut k, &model_res_renames, symbols)?;
                    Ok((k, v))
                })
                .collect::<Result<HashMap<_, _>, _>>()?;
            src.clod_decoders = src
                .clod_decoders
                .into_iter()
                .map(|(mut k, v)| -> Result<_, String> {
                    remap(&mut k, &model_res_renames, symbols)?;
                    Ok((k, v))
                })
                .collect::<Result<HashMap<_, _>, _>>()?;
            src.texture_images = src
                .texture_images
                .into_iter()
                .map(|(mut k, v)| -> Result<_, String> {
                    remap(&mut k, &texture_renames, symbols)?;
                    Ok((k, v))
                })
                .collect::<Result<HashMap<_, _>, _>>()?;
        }

        // Splice the assets in. With generateUniqueNames the names are now
        // distinct so these all append/insert; without it, a same-named
        // incoming element replaces the existing one.
        merge_named(&mut self.materials, src.materials, |m| &m.name, symbols)?;
        merge_named(&mut self.shaders, src.shaders, |s| &s.name, symbols)?;
        merge_named(&mut self.nodes, src.nodes, |n| &n.name, symbols)?;
        merge_named(&mut self.lights, src.lights, |l| &l.name, symbols)?;
        merge_named(&mut self.texture_infos, src.texture_infos, |t| &t.name, symbols)?;
        merge_named(&mut self.skeletons, src.skeletons, |s| &s.name, symbols)?;
        merge_named(&mut self.motions, src.motions, |m| &m.name, symbols)?;
        merge_named(&mut self.raw_meshes, src.raw_meshes, |m| &m.name, symbols)?;
        self.texture_images.extend(src.texture_images);
        self.model_resources.extend(src.model_resources);
        self.clod_meshes.extend(src.clod_meshes);
        self.clod_decoders.extend(src.clod_decoders);
        // Carry the folded biped COM across, or a merged-in skinned model would keep
        // the composed node transform while the renderer stopped stripping it.
        self.model_root_com.extend(src.model_root_com);

        // Force the renderer to re-upload geometry and textures.
        self.mesh_content_version = self.mesh_content_version.wrapping_add(1);
        self.texture_content_version = self.texture_content_version.wrapping_add(1);
        Ok(())
    }
}
