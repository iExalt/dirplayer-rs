//! Lingo Transform object handler.
//! A Transform is a mutable 4x4 row-major matrix used for 3D position/rotation/scale.

use log::debug;

pub(super) fn validate_transform_ref(
    player: &crate::player::DirPlayer,
    datum_ref: &crate::player::DatumRef,
) -> Result<(), crate::player::ScriptError> {
    let Some(owner) = datum_ref.owner() else {
        return Err(crate::player::ScriptError::new_code(
            crate::player::ScriptErrorCode::InvalidReference,
            "invalid Transform3d datum reference".to_owned(),
        ));
    };
    if !owner.same_identity(&player.owner)
        || !owner.is_arena_live()
        || player.allocator.try_get_datum(datum_ref).is_none()
    {
        return Err(crate::player::ScriptError::new_code(
            crate::player::ScriptErrorCode::InvalidReference,
            "foreign or stale Transform3d datum reference".to_owned(),
        ));
    }
    Ok(())
}

pub fn mark_transform_dirty(
    player: &mut crate::player::DirPlayer,
    datum_ref: &crate::player::DatumRef,
) -> Result<(), crate::player::ScriptError> {
    validate_transform_ref(player, datum_ref)?;
    player.w3d_dirty_transform_ids.insert(datum_ref.unwrap());
    Ok(())
}

fn checked_datum<'a>(
    player: &'a DirPlayer,
    datum_ref: &DatumRef,
) -> Result<&'a Datum, ScriptError> {
    match datum_ref {
        DatumRef::Void => Ok(&Datum::Void),
        _ => player
            .allocator
            .try_get_datum(datum_ref)
            .ok_or_else(|| ScriptError::new_code(
                crate::player::ScriptErrorCode::InvalidReference,
                format!("invalid datum reference {datum_ref}"),
            )),
    }
}

fn checked_arg<'a>(
    player: &'a DirPlayer,
    args: &[DatumRef],
    index: usize,
) -> Result<&'a Datum, ScriptError> {
    args.get(index)
        .ok_or_else(|| ScriptError::new_code(
            crate::player::ScriptErrorCode::InvalidReference,
            format!("missing Transform3d argument {index}"),
        ))
        .and_then(|datum_ref| checked_datum(player, datum_ref))
}

fn mark_after_prepare<T>(
    player: &mut DirPlayer,
    datum: &DatumRef,
    prepared: Result<T, ScriptError>,
) -> Result<T, ScriptError> {
    if let Err(error) = &prepared {
        if error.code == crate::player::ScriptErrorCode::InvalidReference {
            return Err(error.clone());
        }
    }
    mark_transform_dirty(player, datum)?;
    prepared
}

fn prepare_axis_angle(
    player: &DirPlayer,
    value: &Datum,
) -> Result<Option<([f64; 3], f64)>, ScriptError> {
    let Datum::List(_, items, _) = value else {
        return Ok(None);
    };
    if items.len() < 2 {
        return Ok(None);
    }
    let axis = match checked_datum(player, &items[0])? {
        Datum::Vector(axis) => *axis,
        _ => return Err(ScriptError::new("axisAngle: expected vector for axis".into())),
    };
    let angle = checked_datum(player, &items[1])?.to_float()?;
    Ok(Some((axis, angle)))
}

enum RotatePreparation {
    Pivot([f64; 3], [f64; 3], f64),
    Euler(f64, f64, f64),
}

fn prepare_rotate(
    player: &DirPlayer,
    args: &[DatumRef],
) -> Result<RotatePreparation, ScriptError> {
    if args.len() >= 3 {
        if matches!(checked_arg(player, args, 0)?, Datum::Vector(_)) {
            let pivot = match checked_arg(player, args, 0)? {
                Datum::Vector(vector) => *vector,
                _ => unreachable!(),
            };
            let axis = match checked_arg(player, args, 1)? {
                Datum::Vector(axis) => *axis,
                _ => [0.0, 0.0, 1.0],
            };
            let angle = checked_arg(player, args, 2)?.to_float()?;
            return Ok(RotatePreparation::Pivot(pivot, axis, angle));
        }
    }
    let (x, y, z) = Transform3dDatumHandlers::read_xyz(player, args)?;
    Ok(RotatePreparation::Euler(x, y, z))
}

use crate::{
    director::lingo::datum::Datum,
    player::{DatumRef, DirPlayer, ScriptError, session::ExecutionContext, symbols::{builtin::BuiltInSymbol, symbol::Symbol, symbol_table::SymbolTable}},
};

const IDENTITY: [f64; 16] = [
    1.0, 0.0, 0.0, 0.0,
    0.0, 1.0, 0.0, 0.0,
    0.0, 0.0, 1.0, 0.0,
    0.0, 0.0, 0.0, 1.0,
];

pub struct Transform3dDatumHandlers;

impl Transform3dDatumHandlers {
    pub fn get_prop(player: &mut DirPlayer, symbols: &SymbolTable, datum: &DatumRef, prop: Symbol) -> Result<Datum, ScriptError> {
        let datum = match datum {
            DatumRef::Void => &Datum::Void,
            _ => player
                .allocator
                .try_get_datum(datum)
                .ok_or_else(|| ScriptError::new(format!("invalid datum reference {datum}")))?,
        };
        let m = match datum {
            Datum::Transform3d(m) => **m,
            _ => return Err(ScriptError::new("Expected Transform3d".into())),
        };
        let prop_name = symbols
            .display(&prop)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;

        match prop.into_builtin() {
            Some(BuiltInSymbol::Position) => Ok(Datum::Vector([m[12], m[13], m[14]])),
            Some(BuiltInSymbol::Rotation) => {
                let (rx, ry, rz) = matrix_to_euler(&m);
                Ok(Datum::Vector([rx, ry, rz]))
            }
            Some(BuiltInSymbol::Scale) => {
                let sx = (m[0]*m[0] + m[1]*m[1] + m[2]*m[2]).sqrt();
                let sy = (m[4]*m[4] + m[5]*m[5] + m[6]*m[6]).sqrt();
                let sz = (m[8]*m[8] + m[9]*m[9] + m[10]*m[10]).sqrt();
                Ok(Datum::Vector([sx, sy, sz]))
            }
            // Director returns the rotation basis as UNIT vectors (scale removed).
            // A scaled node must still report unit axes — e.g. Rasterwerks' actor-model
            // group is scaled ~1.06, and the bot's turn convergence does
            // `targetZaxis.angleBetween(model.transform.zAxis)`; a non-unit zAxis skews
            // that angle so the body settles facing the wrong way. (The control node is
            // unit-scaled, which is why movement was correct but the visible model wasn't.)
            Some(BuiltInSymbol::XAxis) => Ok(Datum::Vector(normalize_vec3([m[0], m[1], m[2]]))),
            Some(BuiltInSymbol::YAxis) => Ok(Datum::Vector(normalize_vec3([m[4], m[5], m[6]]))),
            Some(BuiltInSymbol::ZAxis) => Ok(Datum::Vector(normalize_vec3([m[8], m[9], m[10]]))),
            Some(BuiltInSymbol::AxisAngle) => {
                // Extract axis-angle from the rotation part of the matrix
                let (axis, angle) = matrix_to_axis_angle(&m);
                let axis_ref = player.alloc_datum(Datum::Vector(axis));
                let angle_ref = player.alloc_datum(Datum::Float(angle));
                Ok(Datum::List(
                    crate::director::lingo::datum::DatumType::List,
                    std::collections::VecDeque::from([axis_ref, angle_ref]),
                    false,
                ))
            }
            _ => Err(ScriptError::new(format!("Unknown transform property '{prop_name}'"))),
        }
    }

    pub fn set_prop(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        prop: Symbol,
        value: &DatumRef,
    ) -> Result<(), ScriptError> {
        validate_transform_ref(player, datum)?;
        let prop_name = symbols
            .display(&prop)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        let val = checked_datum(player, value)?.clone();
        let prop_kind = prop.into_builtin();
        let prepared_axis_angle = if matches!(prop_kind, Some(BuiltInSymbol::AxisAngle)) {
            prepare_axis_angle(player, &val)
        } else {
            Ok(None)
        };
        let prepared_axis_angle = mark_after_prepare(player, datum, prepared_axis_angle)?;
        let m = match player.get_datum_mut(datum) {
            Datum::Transform3d(m) => m,
            _ => return Err(ScriptError::new("Expected Transform3d".into())),
        };

        match prop_kind {
            Some(BuiltInSymbol::Position) => {
                if let Datum::Vector(v) = val {
                    // Guard: only set finite values
                    if v[0].is_finite() { m[12] = v[0]; }
                    if v[1].is_finite() { m[13] = v[1]; }
                    if v[2].is_finite() { m[14] = v[2]; }
                    // Debug: log position sets with large Z (overlay models at Z≈-500)
                    if v[2].abs() > 400.0 {
                        static T3D_LOG: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                        if T3D_LOG.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 3 {
                            debug!(
                                "[T3D-POS] transform.position = ({:.1},{:.1},{:.1}) datum_id={:?}",
                                v[0], v[1], v[2], datum
                            );
                        }
                    }
                }
                Ok(())
            }
            Some(BuiltInSymbol::Rotation) => {
                if let Datum::Vector(v) = val {
                    // Log non-zero Z rotation (steering)
                    if v[2].abs() > 0.1 {
                        static ROT_LOG: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                        if ROT_LOG.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 5 {
                            debug!(
                                "[T3D-ROT] transform.rotation = ({:.1},{:.1},{:.1}) pos=({:.1},{:.1},{:.1})",
                                v[0], v[1], v[2], m[12], m[13], m[14]
                            );
                        }
                    }
                    // Preserve position and scale, rebuild rotation
                    let pos = [
                        if m[12].is_finite() { m[12] } else { 0.0 },
                        if m[13].is_finite() { m[13] } else { 0.0 },
                        if m[14].is_finite() { m[14] } else { 0.0 },
                    ];
                    // Guard: if current matrix has NaN, use scale 1.0
                    let sx = (m[0]*m[0] + m[1]*m[1] + m[2]*m[2]).sqrt();
                    let sy = (m[4]*m[4] + m[5]*m[5] + m[6]*m[6]).sqrt();
                    let sz = (m[8]*m[8] + m[9]*m[9] + m[10]*m[10]).sqrt();
                    let sx = if sx.is_finite() && sx > 1e-10 { sx } else { 1.0 };
                    let sy = if sy.is_finite() && sy > 1e-10 { sy } else { 1.0 };
                    let sz = if sz.is_finite() && sz > 1e-10 { sz } else { 1.0 };
                    let rot = euler_to_matrix(v[0], v[1], v[2]);
                    // Apply scale to rotation columns
                    m[0] = rot[0]*sx;  m[1] = rot[1]*sx;  m[2] = rot[2]*sx;
                    m[4] = rot[4]*sy;  m[5] = rot[5]*sy;  m[6] = rot[6]*sy;
                    m[8] = rot[8]*sz;  m[9] = rot[9]*sz;  m[10] = rot[10]*sz;
                    m[12] = pos[0]; m[13] = pos[1]; m[14] = pos[2];
                }
                Ok(())
            }
            Some(BuiltInSymbol::Scale) => {
                if let Datum::Vector(v) = val {
                    // Normalize existing rotation columns, then apply new scale
                    let cur_sx = (m[0]*m[0] + m[1]*m[1] + m[2]*m[2]).sqrt();
                    let cur_sy = (m[4]*m[4] + m[5]*m[5] + m[6]*m[6]).sqrt();
                    let cur_sz = (m[8]*m[8] + m[9]*m[9] + m[10]*m[10]).sqrt();
                    if cur_sx > 0.0 { let s = v[0] / cur_sx; m[0] *= s; m[1] *= s; m[2] *= s; }
                    if cur_sy > 0.0 { let s = v[1] / cur_sy; m[4] *= s; m[5] *= s; m[6] *= s; }
                    if cur_sz > 0.0 { let s = v[2] / cur_sz; m[8] *= s; m[9] *= s; m[10] *= s; }
                }
                Ok(())
            }
            Some(BuiltInSymbol::AxisAngle) => {
                // axisAngle = [vector(axis), angle_degrees]
                let (axis, angle_deg) = prepared_axis_angle
                    .map(|(axis, angle)| (Some(axis), angle))
                    .unwrap_or((None, 0.0));

                if let Some(axis) = axis {
                    let m = match player.get_datum_mut(datum) {
                        Datum::Transform3d(m) => m,
                        _ => return Err(ScriptError::new("Expected Transform3d".into())),
                    };
                    let pos = [m[12], m[13], m[14]];
                    let sx = (m[0]*m[0] + m[1]*m[1] + m[2]*m[2]).sqrt();
                    let sy = (m[4]*m[4] + m[5]*m[5] + m[6]*m[6]).sqrt();
                    let sz = (m[8]*m[8] + m[9]*m[9] + m[10]*m[10]).sqrt();
                    let sx = if sx.is_finite() && sx > 1e-10 { sx } else { 1.0 };
                    let sy = if sy.is_finite() && sy > 1e-10 { sy } else { 1.0 };
                    let sz = if sz.is_finite() && sz > 1e-10 { sz } else { 1.0 };
                    let rot = axis_angle_to_matrix(&axis, angle_deg);
                    m[0] = rot[0]*sx;  m[1] = rot[1]*sx;  m[2] = rot[2]*sx;  m[3] = 0.0;
                    m[4] = rot[4]*sy;  m[5] = rot[5]*sy;  m[6] = rot[6]*sy;  m[7] = 0.0;
                    m[8] = rot[8]*sz;  m[9] = rot[9]*sz;  m[10] = rot[10]*sz; m[11] = 0.0;
                    m[12] = pos[0]; m[13] = pos[1]; m[14] = pos[2]; m[15] = 1.0;
                }
                Ok(())
            }
            _ => Err(ScriptError::new(format!("Cannot set transform property '{prop_name}'"))),
        }
    }

    pub fn call(
        runtime: &mut ExecutionContext<'_>,
        datum: DatumRef,
        handler_name: Symbol,
        args: &[DatumRef],
    ) -> Result<DatumRef, ScriptError> {
        let player = &mut runtime.player;
        let symbols = &mut *runtime.symbols;
        checked_datum(player, &datum)?;
        let handler_name_display = symbols
            .display(&handler_name)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        let is_invert = handler_name_display.eq_ignore_ascii_case("invert");
        if is_invert {
            return Self::invert(player, &datum);
        }
        match handler_name.into_builtin() {
            Some(BuiltInSymbol::Identity) => Self::identity(player, &datum),
            Some(BuiltInSymbol::Translate) => Self::translate(player, &datum, args, true),
            Some(BuiltInSymbol::PreTranslate) => Self::translate(player, &datum, args, false),
            Some(BuiltInSymbol::Rotate) => Self::rotate(player, &datum, args, true),
            Some(BuiltInSymbol::PreRotate) => Self::rotate(player, &datum, args, false),
            Some(BuiltInSymbol::Scale) => Self::scale(player, &datum, args, true),
            Some(BuiltInSymbol::PreScale) => Self::scale(player, &datum, args, false),
            Some(BuiltInSymbol::Inverse) => Self::inverse(player, &datum),
            Some(BuiltInSymbol::Duplicate) => Self::duplicate(player, &datum),
            Some(BuiltInSymbol::Multiply) => Self::multiply(player, &datum, args),
            Some(BuiltInSymbol::Interpolate) => Self::interpolate(player, &datum, args),
            Some(BuiltInSymbol::InterpolateTo) => Self::interpolate_to(player, &datum, args),
            Some(BuiltInSymbol::GetAt) => Self::get_at(player, &datum, args),
            Some(BuiltInSymbol::SetAt) => Self::set_at(player, &datum, args),
            Some(BuiltInSymbol::GetProp | BuiltInSymbol::GetPropRef) => {
                // transform.rotation[3] -> getProp(#rotation, 3)
                let prop_name = checked_arg(player, args, 0)?.symbol_value(symbols)?;
                let prop_datum = Self::get_prop(player, symbols, &datum, prop_name)?;
                if args.len() > 1 {
                    let index = checked_arg(player, args, 1)?.int_value()?;
                    let prop_ref = player.alloc_datum(prop_datum);
                    let prop_val = player.get_datum(&prop_ref).clone();
                    match prop_val {
                        Datum::Vector(v) => {
                            let idx = (index as usize).saturating_sub(1);
                            if idx < 3 { Ok(player.alloc_datum(Datum::Float(v[idx]))) }
                            else { Ok(player.alloc_datum(Datum::Float(0.0))) }
                        }
                        Datum::List(_, items, _) => {
                            let idx = (index as usize).saturating_sub(1);
                            if idx < items.len() { Ok(items[idx].clone()) }
                            else { Ok(DatumRef::Void) }
                        }
                        other => Ok(player.alloc_datum(other)),
                    }
                } else {
                    Ok(player.alloc_datum(prop_datum))
                }
            }
            Some(BuiltInSymbol::Count) => {
                let prop_name = checked_arg(player, args, 0)?.symbol_value(symbols)?;
                let prop_datum = Self::get_prop(player, symbols, &datum, prop_name)?;
                let count = match &prop_datum {
                    Datum::Vector(_) => 3,
                    Datum::List(_, items, _) => items.len() as i32,
                    _ => 1,
                };
                Ok(player.alloc_datum(Datum::Int(count)))
            }
            _ => Err(ScriptError::new(format!("No handler '{}' for transform", handler_name_display))),
        }
    }

    fn identity(player: &mut DirPlayer, datum: &DatumRef) -> Result<DatumRef, ScriptError> {
        mark_transform_dirty(player, datum)?;
        player.allocator.replace_transform3d(datum, IDENTITY)?;
        Ok(DatumRef::Void)
    }

    fn translate(player: &mut DirPlayer, datum: &DatumRef, args: &[DatumRef], pre: bool) -> Result<DatumRef, ScriptError> {
            let xyz = Self::read_xyz(player, args);
            let (dx, dy, dz) = mark_after_prepare(player, datum, xyz)?;
            let m = match player.get_datum(datum) {
                Datum::Transform3d(m) => **m,
                _ => return Err(ScriptError::new("Expected Transform3d".into())),
            };
    
            let t = [
                1.0, 0.0, 0.0, 0.0,
                0.0, 1.0, 0.0, 0.0,
                0.0, 0.0, 1.0, 0.0,
                dx,  dy,  dz,  1.0,
            ];
    
            let result = if pre { mat4_mul(&t, &m) } else { mat4_mul(&m, &t) };
            player.allocator.replace_transform3d(datum, result)?;
            Ok(DatumRef::Void)
    }
    
    fn rotate(player: &mut DirPlayer, datum: &DatumRef, args: &[DatumRef], pre: bool) -> Result<DatumRef, ScriptError> {
        let prepared = checked_datum(player, datum).and_then(|receiver| {
            let m = match receiver {
                Datum::Transform3d(m) => **m,
                _ => return Err(ScriptError::new("Expected Transform3d".into())),
            };
            prepare_rotate(player, args).map(|args| (m, args))
        });
        let (m, prepared) = mark_after_prepare(player, datum, prepared)?;
    
            // Two forms:
            // 1. rotate(rx, ry, rz) or rotate(vector) — Euler angles
            // 2. rotate(point, axis, angle) — rotate around point by angle about axis
            let result = match prepared {
                RotatePreparation::Pivot(pivot, axis, angle_deg) => {
                let r = axis_angle_to_matrix(&axis, angle_deg);
                // P = T(pivot) * R * T(-pivot) — the rotation about `pivot`.
                // rotate(pivot,...)    : world-frame pivot → P * M
                // preRotate(pivot,...) : object-frame pivot → M * P
                // Previously the pivot path always did P * M, which made
                // ClubMarian's CameraOrbit.transform.preRotate(...) treat the
                // pivot as a world point — the orbit wobbled around the world
                // origin instead of swinging around the player's local axis,
                // so the camera never made it behind the avatar after Avatar
                // Options closed.
                let t_neg = translation_matrix(-pivot[0], -pivot[1], -pivot[2]);
                let t_pos = translation_matrix(pivot[0], pivot[1], pivot[2]);
                let p = mat4_mul(&t_pos, &mat4_mul(&r, &t_neg));
                if pre { mat4_mul(&p, &m) } else { mat4_mul(&m, &p) }
                }
                RotatePreparation::Euler(rx, ry, rz) => {
                let r = euler_to_matrix(rx, ry, rz);
                if pre { mat4_mul(&r, &m) } else { mat4_mul(&m, &r) }
                }
            };
    
            player.allocator.replace_transform3d(datum, result)?;
            Ok(DatumRef::Void)
    }
    
    fn scale(player: &mut DirPlayer, datum: &DatumRef, args: &[DatumRef], pre: bool) -> Result<DatumRef, ScriptError> {
            let xyz = Self::read_xyz(player, args);
            let (sx, sy, sz) = mark_after_prepare(player, datum, xyz)?;
            let m = match player.get_datum(datum) {
                Datum::Transform3d(m) => **m,
                _ => return Err(ScriptError::new("Expected Transform3d".into())),
            };
    
            let s = [
                sx,  0.0, 0.0, 0.0,
                0.0, sy,  0.0, 0.0,
                0.0, 0.0, sz,  0.0,
                0.0, 0.0, 0.0, 1.0,
            ];
    
            let result = if pre { mat4_mul(&s, &m) } else { mat4_mul(&m, &s) };
            player.allocator.replace_transform3d(datum, result)?;
            Ok(DatumRef::Void)
    }
    
    fn inverse(player: &mut DirPlayer, datum: &DatumRef) -> Result<DatumRef, ScriptError> {
            let m = match player.get_datum(datum) {
                Datum::Transform3d(m) => **m,
                _ => return Err(ScriptError::new("Expected Transform3d".into())),
            };
            let inv = mat4_invert_affine(&m);
            Ok(player.alloc_datum(Datum::transform3d(inv)))
    }
    
    /// invert() inverts the transform IN PLACE (mutates the original), unlike
    /// inverse() which returns a copy (Director Scripting Dictionary).
    fn invert(player: &mut DirPlayer, datum: &DatumRef) -> Result<DatumRef, ScriptError> {
            mark_transform_dirty(player, datum)?;
            let m = match player.get_datum(datum) {
                Datum::Transform3d(m) => **m,
                _ => return Err(ScriptError::new("Expected Transform3d".into())),
            };
            let inv = mat4_invert_affine(&m);
            player.allocator.replace_transform3d(datum, inv)?;
            Ok(DatumRef::Void)
    }
    
    fn duplicate(player: &mut DirPlayer, datum: &DatumRef) -> Result<DatumRef, ScriptError> {
            let m = match player.get_datum(datum) {
                Datum::Transform3d(m) => **m,
                _ => return Err(ScriptError::new("Expected Transform3d".into())),
            };
            Ok(player.alloc_datum(Datum::transform3d(m)))
    }
    
    fn multiply(player: &mut DirPlayer, datum: &DatumRef, args: &[DatumRef]) -> Result<DatumRef, ScriptError> {
            let m = match player.get_datum(datum) {
                Datum::Transform3d(m) => **m,
                _ => return Err(ScriptError::new("Expected Transform3d".into())),
            };
            let other = match checked_arg(player, args, 0)? {
                Datum::Transform3d(m) => **m,
                _ => return Err(ScriptError::new("Expected Transform3d argument".into())),
            };
            let result = mat4_mul(&m, &other);
            Ok(player.alloc_datum(Datum::transform3d(result)))
    }
    
    fn interpolate(player: &mut DirPlayer, datum: &DatumRef, args: &[DatumRef]) -> Result<DatumRef, ScriptError> {
            let m = match player.get_datum(datum) {
                Datum::Transform3d(m) => **m,
                _ => return Err(ScriptError::new("Expected Transform3d".into())),
            };
            let (target, t) = Self::prepare_interpolate(player, args)?;
            // Lerp position/scale, SLERP rotation (Director 11.5 interpolate():
            // "position and rotation"). Element-wise matrix lerp shears rotation.
            let result = interpolate_transform(&m, &target, t);
            Ok(player.alloc_datum(Datum::Transform3d(Box::new(result))))
    }
    
    fn interpolate_to(player: &mut DirPlayer, datum: &DatumRef, args: &[DatumRef]) -> Result<DatumRef, ScriptError> {
        let prepared = checked_datum(player, datum).and_then(|receiver| {
            let m = match receiver {
                Datum::Transform3d(m) => **m,
                _ => return Err(ScriptError::new("Expected Transform3d".into())),
            };
            Self::prepare_interpolate(player, args).map(|args| (m, args))
        });
        let (m, (target, t)) = mark_after_prepare(player, datum, prepared)?;
            // interpolateTo modifies transform1 in place (Director 11.5).
            let result = interpolate_transform(&m, &target, t);
            player.allocator.replace_transform3d(datum, result)?;
            Ok(DatumRef::Void)
    }
    
    fn get_at(player: &mut DirPlayer, datum: &DatumRef, args: &[DatumRef]) -> Result<DatumRef, ScriptError> {
            let m = match player.get_datum(datum) {
                Datum::Transform3d(m) => **m,
                _ => return Err(ScriptError::new("Expected Transform3d".into())),
            };
            let index = (checked_arg(player, args, 0)?.int_value()? - 1) as usize;
            if index >= 16 {
                return Err(ScriptError::new("Transform index out of range".into()));
            }
            Ok(player.alloc_datum(Datum::Float(m[index])))
    }
    
    fn set_at(player: &mut DirPlayer, datum: &DatumRef, args: &[DatumRef]) -> Result<DatumRef, ScriptError> {
            let prepared = Self::prepare_set_at(player, args);
            let (index, value) = mark_after_prepare(player, datum, prepared)?;
            if index >= 16 {
                return Err(ScriptError::new("Transform index out of range".into()));
            }
            if let Datum::Transform3d(m) = player.get_datum_mut(datum) {
                m[index] = value;
            }
            Ok(DatumRef::Void)
    }

    /// Read (x, y, z) from args - either 3 separate floats or a single vector
    fn read_xyz(player: &DirPlayer, args: &[DatumRef]) -> Result<(f64, f64, f64), ScriptError> {
        if args.len() >= 3 {
            let x = checked_arg(player, args, 0)?.float_value()?;
            let y = checked_arg(player, args, 1)?.float_value()?;
            let z = checked_arg(player, args, 2)?.float_value()?;
            Ok((x, y, z))
        } else if args.len() >= 1 {
            match checked_arg(player, args, 0)? {
                Datum::Vector(v) => Ok((v[0], v[1], v[2])),
                _ => {
                    let x = checked_arg(player, args, 0)?.float_value()?;
                    Ok((x, 0.0, 0.0))
                }
            }
        } else {
            Ok((0.0, 0.0, 0.0))
        }
    }

    fn prepare_interpolate(
        player: &DirPlayer,
        args: &[DatumRef],
    ) -> Result<([f64; 16], f64), ScriptError> {
        let target = match checked_arg(player, args, 0)? {
            Datum::Transform3d(matrix) => **matrix,
            _ => return Err(ScriptError::new("Expected Transform3d argument".into())),
        };
        let percent = checked_arg(player, args, 1)?.float_value()?;
        Ok((target, percent / 100.0))
    }

    fn prepare_set_at(
        player: &DirPlayer,
        args: &[DatumRef],
    ) -> Result<(usize, f64), ScriptError> {
        let index = (checked_arg(player, args, 0)?.int_value()? - 1) as usize;
        let value = checked_arg(player, args, 1)?.float_value()?;
        Ok((index, value))
    }
}

// ─── Matrix math ───

/// Column-major 4x4 matrix multiply: C = A * B
fn mat4_mul(a: &[f64; 16], b: &[f64; 16]) -> [f64; 16] {
    let mut r = [0.0f64; 16];
    for col in 0..4 {
        for row in 0..4 {
            r[col * 4 + row] =
                a[0 * 4 + row] * b[col * 4 + 0] +
                a[1 * 4 + row] * b[col * 4 + 1] +
                a[2 * 4 + row] * b[col * 4 + 2] +
                a[3 * 4 + row] * b[col * 4 + 3];
        }
    }
    r
}

/// Invert a column-major affine transform
fn mat4_invert_affine(m: &[f64; 16]) -> [f64; 16] {
    // Column-major: R[row][col] = m[col*4 + row]
    let (tx, ty, tz) = (m[12], m[13], m[14]);
    // -R^T * t
    let itx = -(m[0] * tx + m[1] * ty + m[2] * tz);
    let ity = -(m[4] * tx + m[5] * ty + m[6] * tz);
    let itz = -(m[8] * tx + m[9] * ty + m[10] * tz);
    [
        m[0], m[4], m[8],  0.0,  // R^T col 0
        m[1], m[5], m[9],  0.0,  // R^T col 1
        m[2], m[6], m[10], 0.0,  // R^T col 2
        itx,  ity,  itz,   1.0,
    ]
}

/// Pure translation matrix (column-major).
fn translation_matrix(tx: f64, ty: f64, tz: f64) -> [f64; 16] {
    [
        1.0, 0.0, 0.0, 0.0,
        0.0, 1.0, 0.0, 0.0,
        0.0, 0.0, 1.0, 0.0,
        tx,  ty,  tz,  1.0,
    ]
}

/// Normalize a 3-vector to unit length (returns the input unchanged if ~zero).
fn normalize_vec3(v: [f64; 3]) -> [f64; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 1e-10 {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        v
    }
}

/// Euler angles to column-major rotation matrix (R = Rz * Ry * Rx)
/// Interpolate two Director (column-major) transforms by `t` (0..1): lerp the
/// position and per-axis scale, SLERP the rotation. Matches the 11.5 dictionary
/// for interpolate()/interpolateTo() ("position and rotation"). An element-wise
/// 16-cell matrix lerp shears the rotation basis — a follow camera would track
/// position but never actually rotate to face its target.
fn interpolate_transform(m: &[f64; 16], target: &[f64; 16], t: f64) -> [f64; 16] {
    let t = t.clamp(0.0, 1.0);
    let (p1, s1, q1) = decompose_transform(m);
    let (p2, s2, q2) = decompose_transform(target);
    let pos = [p1[0] + (p2[0]-p1[0])*t, p1[1] + (p2[1]-p1[1])*t, p1[2] + (p2[2]-p1[2])*t];
    let scale = [s1[0] + (s2[0]-s1[0])*t, s1[1] + (s2[1]-s1[1])*t, s1[2] + (s2[2]-s1[2])*t];
    let q = quat_slerp(q1, q2, t);
    recompose_transform(pos, scale, q)
}

/// Decompose a column-major transform into (position, per-axis scale, rotation
/// quaternion [x,y,z,w]). Scale = column lengths; rotation = normalized columns.
fn decompose_transform(m: &[f64; 16]) -> ([f64; 3], [f64; 3], [f64; 4]) {
    let pos = [m[12], m[13], m[14]];
    let s0 = (m[0]*m[0] + m[1]*m[1] + m[2]*m[2]).sqrt();
    let s1 = (m[4]*m[4] + m[5]*m[5] + m[6]*m[6]).sqrt();
    let s2 = (m[8]*m[8] + m[9]*m[9] + m[10]*m[10]).sqrt();
    let (i0, i1, i2) = (s0.max(1e-10), s1.max(1e-10), s2.max(1e-10));
    // r_{row,col} from normalized columns
    let (r00, r10, r20) = (m[0]/i0, m[1]/i0, m[2]/i0);
    let (r01, r11, r21) = (m[4]/i1, m[5]/i1, m[6]/i1);
    let (r02, r12, r22) = (m[8]/i2, m[9]/i2, m[10]/i2);
    let q = mat3_to_quat(r00, r10, r20, r01, r11, r21, r02, r12, r22);
    (pos, [s0, s1, s2], q)
}

#[allow(clippy::too_many_arguments)]
fn mat3_to_quat(
    r00: f64, r10: f64, r20: f64,
    r01: f64, r11: f64, r21: f64,
    r02: f64, r12: f64, r22: f64,
) -> [f64; 4] {
    let trace = r00 + r11 + r22;
    if trace > 0.0 {
        let s = 0.5 / (trace + 1.0).sqrt();
        normalize_quat([(r21 - r12) * s, (r02 - r20) * s, (r10 - r01) * s, 0.25 / s])
    } else if r00 > r11 && r00 > r22 {
        let s = (2.0 * (1.0 + r00 - r11 - r22).sqrt()).max(1e-10);
        normalize_quat([0.25 * s, (r01 + r10) / s, (r02 + r20) / s, (r21 - r12) / s])
    } else if r11 > r22 {
        let s = (2.0 * (1.0 + r11 - r00 - r22).sqrt()).max(1e-10);
        normalize_quat([(r01 + r10) / s, 0.25 * s, (r12 + r21) / s, (r02 - r20) / s])
    } else {
        let s = (2.0 * (1.0 + r22 - r00 - r11).sqrt()).max(1e-10);
        normalize_quat([(r02 + r20) / s, (r12 + r21) / s, 0.25 * s, (r10 - r01) / s])
    }
}

fn recompose_transform(pos: [f64; 3], scale: [f64; 3], q: [f64; 4]) -> [f64; 16] {
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    let (r00, r10, r20) = (1.0 - 2.0*(y*y + z*z), 2.0*(x*y + z*w), 2.0*(x*z - y*w));
    let (r01, r11, r21) = (2.0*(x*y - z*w), 1.0 - 2.0*(x*x + z*z), 2.0*(y*z + x*w));
    let (r02, r12, r22) = (2.0*(x*z + y*w), 2.0*(y*z - x*w), 1.0 - 2.0*(x*x + y*y));
    [
        r00*scale[0], r10*scale[0], r20*scale[0], 0.0,
        r01*scale[1], r11*scale[1], r21*scale[1], 0.0,
        r02*scale[2], r12*scale[2], r22*scale[2], 0.0,
        pos[0], pos[1], pos[2], 1.0,
    ]
}

fn quat_slerp(q1: [f64; 4], mut q2: [f64; 4], t: f64) -> [f64; 4] {
    let mut dot = q1[0]*q2[0] + q1[1]*q2[1] + q1[2]*q2[2] + q1[3]*q2[3];
    if dot < 0.0 {
        for i in 0..4 { q2[i] = -q2[i]; }
        dot = -dot;
    }
    if dot > 0.9995 {
        // nearly parallel → lerp + normalize (avoids sin(0) blow-up)
        return normalize_quat([
            q1[0] + (q2[0]-q1[0])*t, q1[1] + (q2[1]-q1[1])*t,
            q1[2] + (q2[2]-q1[2])*t, q1[3] + (q2[3]-q1[3])*t,
        ]);
    }
    let theta = dot.clamp(-1.0, 1.0).acos();
    let s = theta.sin();
    let a = ((1.0 - t) * theta).sin() / s;
    let b = (t * theta).sin() / s;
    normalize_quat([
        q1[0]*a + q2[0]*b, q1[1]*a + q2[1]*b,
        q1[2]*a + q2[2]*b, q1[3]*a + q2[3]*b,
    ])
}

fn normalize_quat(q: [f64; 4]) -> [f64; 4] {
    let len = (q[0]*q[0] + q[1]*q[1] + q[2]*q[2] + q[3]*q[3]).sqrt();
    if len > 1e-10 { [q[0]/len, q[1]/len, q[2]/len, q[3]/len] } else { [0.0, 0.0, 0.0, 1.0] }
}

pub fn euler_to_matrix(rx_deg: f64, ry_deg: f64, rz_deg: f64) -> [f64; 16] {
    // Guard against NaN/infinity — use 0 for any invalid input
    let rx = if rx_deg.is_finite() { rx_deg } else { 0.0 }.to_radians();
    let ry = if ry_deg.is_finite() { ry_deg } else { 0.0 }.to_radians();
    let rz = if rz_deg.is_finite() { rz_deg } else { 0.0 }.to_radians();

    let (sx, cx) = (rx.sin(), rx.cos());
    let (sy, cy) = (ry.sin(), ry.cos());
    let (sz, cz) = (rz.sin(), rz.cos());

    // R = Rz * Ry * Rx, true column-major: m[col*4+row]
    [
        cy*cz,                     cy*sz,                     -sy,                     0.0,  // col 0
        sx*sy*cz - cx*sz,          sx*sy*sz + cx*cz,          sx*cy,                   0.0,  // col 1
        cx*sy*cz + sx*sz,          cx*sy*sz - sx*cz,          cx*cy,                   0.0,  // col 2
        0.0,                       0.0,                       0.0,                     1.0,  // col 3
    ]
}

/// Extract euler angles from rotation matrix (matching euler_to_matrix convention)
fn matrix_to_euler(m: &[f64; 16]) -> (f64, f64, f64) {
    // Normalize rotation columns to remove scale before extracting angles
    let s0 = (m[0]*m[0] + m[1]*m[1] + m[2]*m[2]).sqrt().max(1e-10);
    let s1 = (m[4]*m[4] + m[5]*m[5] + m[6]*m[6]).sqrt().max(1e-10);
    let s2 = (m[8]*m[8] + m[9]*m[9] + m[10]*m[10]).sqrt().max(1e-10);
    let n = [m[0]/s0, m[1]/s0, m[2]/s0, 0.0,
             m[4]/s1, m[5]/s1, m[6]/s1, 0.0,
             m[8]/s2, m[9]/s2, m[10]/s2, 0.0,
             0.0, 0.0, 0.0, 1.0];

    // Guard: if matrix contains NaN, return zero rotation
    if !n[0].is_finite() || !n[2].is_finite() || !n[10].is_finite() {
        return (0.0, 0.0, 0.0);
    }

    let sy = (-n[2]).clamp(-1.0, 1.0);
    let ry = sy.asin();
    let cy = ry.cos();

    let (rx, rz);
    if cy.abs() > 1e-6 {
        rx = (n[6] / cy).atan2(n[10] / cy);
        rz = (n[1] / cy).atan2(n[0] / cy);
    } else {
        rx = 0.0;
        rz = n[4].atan2(n[5]);
    }

    (rx.to_degrees(), ry.to_degrees(), rz.to_degrees())
}

/// Extract axis-angle representation from the rotation part of a 4x4 matrix.
/// Returns (axis [f64; 3], angle_degrees f64).
fn matrix_to_axis_angle(m: &[f64; 16]) -> ([f64; 3], f64) {
    // Normalize rotation columns to remove scale
    let s0 = (m[0]*m[0] + m[1]*m[1] + m[2]*m[2]).sqrt().max(1e-10);
    let s1 = (m[4]*m[4] + m[5]*m[5] + m[6]*m[6]).sqrt().max(1e-10);
    let s2 = (m[8]*m[8] + m[9]*m[9] + m[10]*m[10]).sqrt().max(1e-10);
    let r00 = m[0]/s0; let r01 = m[1]/s0; let r02 = m[2]/s0;
    let r10 = m[4]/s1; let r11 = m[5]/s1; let r12 = m[6]/s1;
    let r20 = m[8]/s2; let r21 = m[9]/s2; let r22 = m[10]/s2;

    // trace = 1 + 2*cos(angle)
    let trace = r00 + r11 + r22;
    let cos_a = ((trace - 1.0) / 2.0).clamp(-1.0, 1.0);
    let angle = cos_a.acos(); // radians

    if angle.abs() < 1e-10 {
        // No rotation
        return ([1.0, 0.0, 0.0], 0.0);
    }

    let sin_a = angle.sin();
    if sin_a.abs() > 1e-10 {
        let k = 1.0 / (2.0 * sin_a);
        let axis = [
            (r21 - r12) * k,
            (r02 - r20) * k,
            (r10 - r01) * k,
        ];
        (axis, angle.to_degrees())
    } else {
        // angle ≈ 180°, need to extract axis from the matrix diagonal
        let (ax, ay, az) = if r00 >= r11 && r00 >= r22 {
            let x = ((r00 + 1.0) / 2.0).sqrt();
            (x, r01 / (2.0 * x), r02 / (2.0 * x))
        } else if r11 >= r22 {
            let y = ((r11 + 1.0) / 2.0).sqrt();
            (r01 / (2.0 * y), y, r12 / (2.0 * y))
        } else {
            let z = ((r22 + 1.0) / 2.0).sqrt();
            (r02 / (2.0 * z), r12 / (2.0 * z), z)
        };
        ([ax, ay, az], angle.to_degrees())
    }
}

/// Build a 4x4 rotation matrix from axis-angle (angle in degrees).
fn axis_angle_to_matrix(axis: &[f64; 3], angle_deg: f64) -> [f64; 16] {
    let len = (axis[0]*axis[0] + axis[1]*axis[1] + axis[2]*axis[2]).sqrt();
    if len < 1e-10 {
        return [1.0,0.0,0.0,0.0, 0.0,1.0,0.0,0.0, 0.0,0.0,1.0,0.0, 0.0,0.0,0.0,1.0];
    }
    let (x, y, z) = (axis[0]/len, axis[1]/len, axis[2]/len);
    let a = angle_deg.to_radians();
    let c = a.cos();
    let s = a.sin();
    let t = 1.0 - c;
    [
        t*x*x + c,    t*x*y + s*z,  t*x*z - s*y,  0.0,
        t*x*y - s*z,  t*y*y + c,    t*y*z + s*x,  0.0,
        t*x*z + s*y,  t*y*z - s*x,  t*z*z + c,    0.0,
        0.0,          0.0,          0.0,           1.0,
    ]
}
