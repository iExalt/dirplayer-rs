use crate::{
    director::lingo::datum::Datum,
    player::{
        compare::validate_direct_symbol_fields,
        session::ExecutionContext,
        reserve_player_mut, DatumRef, DirPlayer, ScriptError,
        symbols::{builtin::BuiltInSymbol, symbol::Symbol, symbol_table::SymbolTable},
    },
};

pub struct VectorDatumHandlers {}

impl VectorDatumHandlers {
    /// Convert a Datum (Vector or List) into a [f64;3] array
    fn checked_datum_to_vec(
        player: &DirPlayer,
        symbols: &SymbolTable,
        datum_ref: &DatumRef,
    ) -> Result<[f64; 3], ScriptError> {
        let datum = checked_datum(player, datum_ref, symbols)?;
        match datum {
            Datum::Vector(arr) => Ok(*arr),
            Datum::List(_, list, _) if list.len() == 3 => {
                // Resolve components left-to-right. Every nested reference is
                // checked against this session before conversion.
                let components = [list[0].clone(), list[1].clone(), list[2].clone()];
                let mut values = [0.0; 3];
                for (index, component_ref) in components.iter().enumerate() {
                    values[index] = checked_datum(player, component_ref, symbols)?.float_value()?;
                }
                Ok(values)
            }
            Datum::Void | Datum::Int(0) => Ok([0.0, 0.0, 0.0]),
            _ => Err(ScriptError::new(format!("Expected a vector, got {}", datum.type_str()))),
        }
    }

    /// Legacy arithmetic helpers retain their ambient API until that family
    /// receives its own explicit runtime boundary.
    fn datum_to_vec(player: &DirPlayer, datum: &Datum) -> Result<[f64; 3], ScriptError> {
        match datum {
            Datum::Vector(arr) => Ok(*arr),
            Datum::List(_, list, _) if list.len() == 3 => Ok([
                player.get_datum(&list[0]).float_value()?,
                player.get_datum(&list[1]).float_value()?,
                player.get_datum(&list[2]).float_value()?,
            ]),
            Datum::Void | Datum::Int(0) => Ok([0.0, 0.0, 0.0]),
            _ => Err(ScriptError::new(format!("Expected a vector, got {}", datum.type_str()))),
        }
    }

    /// Convert a [f64;3] array into a Datum::Vector
    fn vec_to_datum(player: &mut DirPlayer, vec: [f64; 3]) -> DatumRef {
        player.alloc_datum(Datum::Vector(vec))
    }

    /// Call a handler by name
    pub fn call(
        runtime: &mut ExecutionContext<'_>,
        datum: DatumRef,
        handler_name: Symbol,
        args: &[DatumRef],
    ) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            // Resolve through the owning table before using the builtin id.
            // This rejects foreign dynamic symbols and keeps diagnostics in
            // the caller's display spelling.
            let handler_name_display = symbols
                .display(&handler_name)
                .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
            let builtin = handler_name.into_builtin();

            match builtin {
            Some(BuiltInSymbol::GetAt) => {
                Self::get_at(player, symbols, &datum, args)
            }
            Some(BuiltInSymbol::SetAt) => {
                Self::set_at(player, symbols, &datum, args)
            }
            Some(BuiltInSymbol::Duplicate) => {
                let vec = Self::checked_datum_to_vec(player, symbols, &datum)?;
                Ok(player.alloc_datum(Datum::Vector(vec)))
            }
            Some(BuiltInSymbol::DistanceTo) => {
                if args.is_empty() {
                    return Err(ScriptError::new("distanceTo requires a vector".to_string()));
                }
                let a = Self::checked_datum_to_vec(player, symbols, &datum)?;
                let b = Self::checked_datum_to_vec(player, symbols, &args[0])?;
                let dx = a[0] - b[0];
                let dy = a[1] - b[1];
                let dz = a[2] - b[2];
                Ok(player.alloc_datum(Datum::Float((dx*dx + dy*dy + dz*dz).sqrt())))
            }
            Some(BuiltInSymbol::GetNormalized) => {
                let [x, y, z] = Self::checked_datum_to_vec(player, symbols, &datum)?;
                let len = (x*x + y*y + z*z).sqrt();
                if len > 1e-10 {
                    Ok(player.alloc_datum(Datum::Vector([x/len, y/len, z/len])))
                } else {
                    Ok(player.alloc_datum(Datum::Vector([0.0, 0.0, 0.0])))
                }
            }
            Some(BuiltInSymbol::Normalize) => {
                let [x, y, z] = Self::checked_datum_to_vec(player, symbols, &datum)?;
                let len = (x*x + y*y + z*z).sqrt();
                if len > 1e-10 {
                    *player
                        .allocator
                        .try_get_datum_mut(&datum)
                        .ok_or_else(|| ScriptError::new(format!("invalid datum reference {datum}")))? =
                        Datum::Vector([x/len, y/len, z/len]);
                }
                Ok(DatumRef::Void)
            }
            Some(BuiltInSymbol::CrossProduct | BuiltInSymbol::Cross) => {
                if args.is_empty() {
                    return Err(ScriptError::new("crossProduct requires a vector".to_string()));
                }
                let a = Self::checked_datum_to_vec(player, symbols, &datum)?;
                let b = Self::checked_datum_to_vec(player, symbols, &args[0])?;
                Ok(player.alloc_datum(Datum::Vector([
                    a[1]*b[2] - a[2]*b[1],
                    a[2]*b[0] - a[0]*b[2],
                    a[0]*b[1] - a[1]*b[0],
                ])))
            }
            Some(BuiltInSymbol::DotProduct | BuiltInSymbol::Dot) => {
                if args.is_empty() {
                    return Err(ScriptError::new("dotProduct requires a vector".to_string()));
                }
                let a = Self::checked_datum_to_vec(player, symbols, &datum)?;
                let b = Self::checked_datum_to_vec(player, symbols, &args[0])?;
                Ok(player.alloc_datum(Datum::Float(a[0]*b[0] + a[1]*b[1] + a[2]*b[2])))
            }
            Some(BuiltInSymbol::AngleBetween) => {
                if args.is_empty() {
                    return Err(ScriptError::new("angleBetween requires a vector".to_string()));
                }
                let a = Self::checked_datum_to_vec(player, symbols, &datum)?;
                let b = Self::checked_datum_to_vec(player, symbols, &args[0])?;
                let len_a = (a[0]*a[0] + a[1]*a[1] + a[2]*a[2]).sqrt();
                let len_b = (b[0]*b[0] + b[1]*b[1] + b[2]*b[2]).sqrt();
                let angle = if len_a > 1e-10 && len_b > 1e-10 {
                    let cos_angle = (a[0]*b[0] + a[1]*b[1] + a[2]*b[2]) / (len_a * len_b);
                    cos_angle.clamp(-1.0, 1.0).acos().to_degrees()
                } else {
                    0.0
                };
                Ok(player.alloc_datum(Datum::Float(angle)))
            }
            _ => Err(ScriptError::new(format!("No handler {handler_name_display} for vector"))),
            }
        })
    }

    /// Get a vector component by index (1-based)
    pub fn get_at(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        args: &[DatumRef],
    ) -> Result<DatumRef, ScriptError> {
        if args.is_empty() {
            return Err(ScriptError::new("getAt requires an index".to_string()));
        }
        let vec = Self::checked_datum_to_vec(player, symbols, datum)?;
        let index = checked_datum(player, &args[0], symbols)?.int_value()?;
        if !(1..=3).contains(&index) {
            return Err(ScriptError::new("Index out of range for vector".to_string()));
        }
        Ok(player.alloc_datum(Datum::Float(vec[(index - 1) as usize])))
    }

    /// Set a vector component by index (1-based)
    pub fn set_at(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        args: &[DatumRef],
    ) -> Result<DatumRef, ScriptError> {
        if args.len() < 2 {
            return Err(ScriptError::new("setAt requires an index and value".to_string()));
        }
        let mut vec = match checked_datum(player, datum, symbols)? {
            Datum::Vector(values) => *values,
            _ => return Err(ScriptError::new("Cannot set prop of non-vector".to_string())),
        };
        let index = checked_datum(player, &args[0], symbols)?.int_value()?;
        if !(1..=3).contains(&index) {
            return Err(ScriptError::new("Index out of range for vector".to_string()));
        }
        let value = checked_datum(player, &args[1], symbols)?.float_value()? as f64;
        vec[(index - 1) as usize] = value;

        *player
            .allocator
            .try_get_datum_mut(datum)
            .ok_or_else(|| ScriptError::new(format!("invalid datum reference {datum}")))? =
            Datum::Vector(vec);
        Ok(DatumRef::Void)
    }

    /// Get a vector property (x, y, z, ilk)
    pub fn get_prop(
        player: &DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        prop: Symbol,
    ) -> Result<Datum, ScriptError> {
        let datum = match datum {
            DatumRef::Void => &Datum::Void,
            _ => player
                .allocator
                .try_get_datum(datum)
                .ok_or_else(|| ScriptError::new(format!("invalid datum reference {datum}")))?,
        };
        let [x, y, z] = match datum {
            Datum::Vector(arr) => *arr,
            Datum::List(_, list, _) if list.len() == 3 => {
                let mut components = [0.0; 3];
                for (index, component_ref) in list.iter().enumerate() {
                    let component = match component_ref {
                        DatumRef::Void => &Datum::Void,
                        _ => player
                            .allocator
                            .try_get_datum(component_ref)
                            .ok_or_else(|| ScriptError::new(format!("invalid datum reference {component_ref}")))?,
                    };
                    crate::player::compare::validate_direct_symbol_fields(component, symbols)?;
                    components[index] = component.float_value()?;
                }
                components
            }
            Datum::Void | Datum::Int(0) => [0.0, 0.0, 0.0],
            _ => return Err(ScriptError::new(format!("Expected a vector, got {}", datum.type_str()))),
        };
        let prop_name = symbols
            .display(&prop)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        match prop.into_builtin() {
            Some(BuiltInSymbol::X) => Ok(Datum::Float(x)),
            Some(BuiltInSymbol::Y) => Ok(Datum::Float(y)),
            Some(BuiltInSymbol::Z) => Ok(Datum::Float(z)),
            Some(BuiltInSymbol::Magnitude | BuiltInSymbol::Length) => Ok(Datum::Float((x * x + y * y + z * z).sqrt())),
            Some(BuiltInSymbol::Ilk) => Ok(Datum::Symbol(BuiltInSymbol::Vector.into())),
            _ => Err(ScriptError::new(format!(
                "Cannot get vector property {}",
                prop_name
            ))),
        }
    }

    /// Set a vector property (x, y, z)
    pub fn set_prop(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        prop: Symbol,
        value_ref: &DatumRef,
    ) -> Result<(), ScriptError> {
        let is_vector = matches!(checked_datum(player, datum, symbols)?, Datum::Vector(_));
        if !is_vector {
            return Err(ScriptError::new("Cannot set prop of non-vector".to_string()));
        }
        let prop_name = symbols
            .display(&prop)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        let value = checked_datum(player, value_ref, symbols)?.float_value()? as f64;
        let prop_kind = prop.into_builtin();
        if !matches!(
            prop_kind,
            Some(BuiltInSymbol::X | BuiltInSymbol::Y | BuiltInSymbol::Z)
        ) {
            return Err(ScriptError::new(format!(
                "Cannot set vector property {}",
                prop_name
            )));
        }
        // Validate a retained parent before mutating the inner vector. The
        // legacy writeback occurs after the vector mutation, so keep the
        // dirty mark there while rejecting foreign/stale parents first.
        let parent_mapping = player
            .transform_sub_refs
            .iter()
            .find(|(vec_ref, _, _)| vec_ref == datum)
            .cloned();
        let parent_sub_prop = if let Some((_, parent_ref, sub_prop)) = &parent_mapping {
            super::transform3d::validate_transform_ref(player, parent_ref)?;
            match player.allocator.try_get_datum(parent_ref) {
                Some(Datum::Transform3d(_)) => Some(
                    symbols
                        .lower(sub_prop)
                        .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
                        .to_owned(),
                ),
                _ => None,
            }
        } else {
            None
        };
        let mut vec = match player.get_datum_mut(datum) {
            Datum::Vector(arr) => *arr,
            _ => {
                return Err(ScriptError::new(
                    "Cannot set prop of non-vector".to_string(),
                ))
            }
        };

        match prop_kind {
            Some(BuiltInSymbol::X) => vec[0] = value,
            Some(BuiltInSymbol::Y) => vec[1] = value,
            Some(BuiltInSymbol::Z) => vec[2] = value,
            _ => {
                return Err(ScriptError::new(format!(
                    "Cannot set vector property {}",
                    prop_name
                )))
            }
        }

        *player.get_datum_mut(datum) = Datum::Vector(vec);

        // Write back to parent transform if this vector came from transform.position/rotation.
        // Mark the parent transform dirty so sync_persistent_transforms propagates the
        // change to node_transforms — otherwise `obj.transform.position.y = X` mutates
        // the datum but the renderer never sees it, because the dirty set only tracks
        // the inner Vector datum (which has no node mapping), not the parent transform.
        if let Some((_, parent_ref, _sub_prop)) = parent_mapping {
            super::transform3d::mark_transform_dirty(player, &parent_ref)?;
            if let Datum::Transform3d(m) = player.get_datum_mut(&parent_ref) {
                match parent_sub_prop.as_deref() {
                    Some("position") => {
                        m[12] = vec[0]; m[13] = vec[1]; m[14] = vec[2];
                    }
                    Some("rotation") => {
                        let pos = [m[12], m[13], m[14]];
                        let sx = (m[0]*m[0] + m[1]*m[1] + m[2]*m[2]).sqrt();
                        let sy = (m[4]*m[4] + m[5]*m[5] + m[6]*m[6]).sqrt();
                        let sz = (m[8]*m[8] + m[9]*m[9] + m[10]*m[10]).sqrt();
                        let rot = crate::player::handlers::datum_handlers::transform3d::euler_to_matrix(vec[0], vec[1], vec[2]);
                        m[0] = rot[0]*sx;  m[1] = rot[1]*sx;  m[2] = rot[2]*sx;
                        m[4] = rot[4]*sy;  m[5] = rot[5]*sy;  m[6] = rot[6]*sy;
                        m[8] = rot[8]*sz;  m[9] = rot[9]*sz;  m[10] = rot[10]*sz;
                        m[12] = pos[0]; m[13] = pos[1]; m[14] = pos[2];
                    }
                    Some("scale") => {
                        // Set column lengths to new scale while preserving rotation direction
                        let old_sx = (m[0]*m[0] + m[1]*m[1] + m[2]*m[2]).sqrt().max(1e-10);
                        let old_sy = (m[4]*m[4] + m[5]*m[5] + m[6]*m[6]).sqrt().max(1e-10);
                        let old_sz = (m[8]*m[8] + m[9]*m[9] + m[10]*m[10]).sqrt().max(1e-10);
                        let fx = vec[0] / old_sx;
                        let fy = vec[1] / old_sy;
                        let fz = vec[2] / old_sz;
                        m[0] *= fx; m[1] *= fx; m[2] *= fx;
                        m[4] *= fy; m[5] *= fy; m[6] *= fy;
                        m[8] *= fz; m[9] *= fz; m[10] *= fz;
                    }
                    _ => {}
                }
            }
        }

        Ok(())
    }

    /// Vector addition
    pub fn add(a: &DatumRef, b: &DatumRef) -> Result<DatumRef, ScriptError> {
        reserve_player_mut(|player| {
            let va = Self::datum_to_vec(player, player.get_datum(a))?;
            let vb = Self::datum_to_vec(player, player.get_datum(b))?;
            Ok(Self::vec_to_datum(
                player,
                [va[0] + vb[0], va[1] + vb[1], va[2] + vb[2]],
            ))
        })
    }

    /// Vector subtraction
    pub fn sub(a: &DatumRef, b: &DatumRef) -> Result<DatumRef, ScriptError> {
        reserve_player_mut(|player| {
            let va = Self::datum_to_vec(player, player.get_datum(a))?;
            let vb = Self::datum_to_vec(player, player.get_datum(b))?;
            Ok(Self::vec_to_datum(
                player,
                [va[0] - vb[0], va[1] - vb[1], va[2] - vb[2]],
            ))
        })
    }

    /// Vector multiplication (scalar or component-wise)
    pub fn mul(a: &DatumRef, b: &DatumRef) -> Result<DatumRef, ScriptError> {
        reserve_player_mut(|player| {
            let va = Self::datum_to_vec(player, player.get_datum(a))?;
            match player.get_datum(b) {
                Datum::Float(f) => Ok(Self::vec_to_datum(
                    player,
                    [
                        va[0] * (*f as f64),
                        va[1] * (*f as f64),
                        va[2] * (*f as f64),
                    ],
                )),
                Datum::Vector(vb) => Ok(Self::vec_to_datum(
                    player,
                    [va[0] * vb[0], va[1] * vb[1], va[2] * vb[2]],
                )),
                _ => Err(ScriptError::new(
                    "Invalid operand for vector multiplication".to_string(),
                )),
            }
        })
    }

    /// Vector division (scalar or component-wise)
    pub fn div(a: &DatumRef, b: &DatumRef) -> Result<DatumRef, ScriptError> {
        reserve_player_mut(|player| {
            let va = Self::datum_to_vec(player, player.get_datum(a))?;
            match player.get_datum(b) {
                Datum::Float(f) => {
                    if *f == 0.0 {
                        return Err(ScriptError::new("Division by zero".to_string()));
                    }
                    Ok(Self::vec_to_datum(
                        player,
                        [
                            va[0] / (*f as f64),
                            va[1] / (*f as f64),
                            va[2] / (*f as f64),
                        ],
                    ))
                }
                Datum::Vector(vb) => {
                    if vb[0] == 0.0 || vb[1] == 0.0 || vb[2] == 0.0 {
                        return Err(ScriptError::new(
                            "Division by zero in vector components".to_string(),
                        ));
                    }
                    Ok(Self::vec_to_datum(
                        player,
                        [va[0] / vb[0], va[1] / vb[1], va[2] / vb[2]],
                    ))
                }
                _ => Err(ScriptError::new(
                    "Invalid operand for vector division".to_string(),
                )),
            }
        })
    }
}

fn checked_datum<'a>(
    player: &'a DirPlayer,
    datum: &DatumRef,
    symbols: &SymbolTable,
) -> Result<&'a Datum, ScriptError> {
    let value = match datum {
        DatumRef::Void => &Datum::Void,
        _ => player
            .allocator
            .try_get_datum(datum)
            .ok_or_else(|| ScriptError::new(format!("invalid datum reference {datum}")))?,
    };
    validate_direct_symbol_fields(value, symbols)?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::VectorDatumHandlers;
    use crate::director::lingo::datum::Datum;
    use crate::player::session::RuntimeSession;
    use crate::player::symbols::symbol_table::SymbolOwner;

    #[test]
    fn parent_writeback_marks_the_parent_transform() {
        let mut session = RuntimeSession::new(SymbolOwner { session: 74, generation: 1 });
        let (tx, _rx) = async_std::channel::unbounded();
        assert!(session.add_player(1, tx));
        let outcome = session.with_player(1, |mut runtime| {
            let parent = runtime.player.alloc_datum(Datum::transform3d([
                1.0, 0.0, 0.0, 0.0,
                0.0, 1.0, 0.0, 0.0,
                0.0, 0.0, 1.0, 0.0,
                0.0, 0.0, 0.0, 1.0,
            ]));
            let vector = runtime.player.alloc_datum(Datum::Vector([1.0, 2.0, 3.0]));
            let value = runtime.player.alloc_datum(Datum::Float(9.0));
            let parent_id = parent.unwrap();
            let _vector_id = vector.unwrap();
            let prop = runtime.symbols.intern("x");
            let sub_prop = runtime.symbols.intern("position");
            runtime.player.transform_sub_refs.push((vector.clone(), parent.clone(), sub_prop));
            VectorDatumHandlers::set_prop(runtime.player, runtime.symbols, &vector, prop, &value)?;
            assert_eq!(runtime.player.w3d_dirty_transform_ids.contains(&parent_id), true);
            let vector_value = match runtime.player.get_datum(&vector) {
                Datum::Vector(value) => *value,
                _ => return Err(crate::player::ScriptError::new("expected vector".into())),
            };
            assert_eq!(vector_value, [9.0, 2.0, 3.0]);
            assert!(!runtime.player.w3d_dirty_transform_ids.contains(&vector.unwrap()));
            let parent_position = match runtime.player.get_datum(&parent) {
                Datum::Transform3d(matrix) => matrix[12],
                _ => return Err(crate::player::ScriptError::new("expected parent transform".into())),
            };
            assert_eq!(parent_position, 9.0);
            Ok::<(), crate::player::ScriptError>(())
        });
        assert!(outcome.expect("test player exists").is_ok());
    }
}
