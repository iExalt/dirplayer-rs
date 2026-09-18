use crate::{
    director::lingo::datum::Datum,
    player::{
        bitmap::bitmap::{
            get_system_default_palette, nearest_palette_index, resolve_color_ref, PaletteRef,
        },
        compare::validate_direct_symbol_fields,
        session::ExecutionContext,
        sprite::ColorRef,
        symbols::{builtin::BuiltInSymbol, symbol::Symbol, symbol_table::SymbolTable},
        DatumRef, DirPlayer, ScriptError,
    },
};

pub struct ColorDatumHandlers {}

impl ColorDatumHandlers {
    pub fn call(
        runtime: &mut ExecutionContext<'_>,
        datum: DatumRef,
        handler_name: Symbol,
        _args: &[DatumRef],
    ) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let handler_name_display = symbols
                .display(&handler_name)
                .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
            let handler_name_lower = symbols
                .lower(&handler_name)
                .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
            let datum_value = checked_datum(player, &datum, symbols)?;
            match handler_name_lower {
                "hexstring" => {
                    let color_ref = datum_value.to_color_ref()?;
                    let (r, g, b) = resolve_color_ref(
                        &player.movie.cast_manager.palettes(),
                        color_ref,
                        &PaletteRef::BuiltIn(get_system_default_palette()),
                        8,
                    );
                    let hex_string = format!("#{:02X}{:02X}{:02X}", r, g, b);
                    Ok(player.alloc_datum(Datum::String(hex_string)))
                }
                "duplicate" => Ok(datum.clone()),
                _ => Err(ScriptError::new(format!(
                    "no handler {handler_name_display} for color"
                ))),
            }
        })
    }

    pub fn get_prop(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        prop: Symbol,
    ) -> Result<DatumRef, ScriptError> {
        let datum = match datum {
            DatumRef::Void => &Datum::Void,
            _ => player
                .allocator
                .try_get_datum(datum)
                .ok_or_else(|| ScriptError::new(format!("invalid datum reference {datum}")))?,
        };
        let color_ref = datum.to_color_ref()?;
        let prop_name = symbols
            .display(&prop)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        let prop_lower = symbols
            .lower(&prop)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        match prop_lower {
            "red" => match color_ref {
                ColorRef::Rgb(r, _, _) => Ok(player.alloc_datum(Datum::Int(*r as i32))),
                ColorRef::PaletteIndex(i) => match i {
                    0 => Ok(player.alloc_datum(Datum::Int(255))),
                    255 => Ok(player.alloc_datum(Datum::Int(0))),
                    _ => Ok(player.alloc_datum(Datum::Int(255))),
                },
            },
            "green" => match color_ref {
                ColorRef::Rgb(_, g, _) => Ok(player.alloc_datum(Datum::Int(*g as i32))),
                ColorRef::PaletteIndex(i) => match i {
                    0 => Ok(player.alloc_datum(Datum::Int(255))),
                    255 => Ok(player.alloc_datum(Datum::Int(0))),
                    _ => Ok(player.alloc_datum(Datum::Int(0))),
                },
            },
            "blue" => match color_ref {
                ColorRef::Rgb(_, _, b) => Ok(player.alloc_datum(Datum::Int(*b as i32))),
                ColorRef::PaletteIndex(i) => match i {
                    0 => Ok(player.alloc_datum(Datum::Int(255))),
                    255 => Ok(player.alloc_datum(Datum::Int(0))),
                    _ => Ok(player.alloc_datum(Datum::Int(255))),
                },
            },
            "ilk" => Ok(player.alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::Color)))),
            "colortype" => {
                match color_ref {
                    ColorRef::Rgb(..) => {
                        Ok(player.alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::Rgb))))
                    }
                    ColorRef::PaletteIndex(_) => Ok(player
                        .alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::PaletteIndex)))),
                }
            }
            "paletteindex" => match color_ref {
                ColorRef::PaletteIndex(i) => Ok(player.alloc_datum(Datum::Int(*i as i32))),
                // Director 11.5 Scripting Dictionary p.832: `.paletteIndex` on
                // an RGB color returns the nearest match in the current palette.
                // E.g. `rgb(0,0,0).paletteIndex` → 255 on SystemWin.
                ColorRef::Rgb(r, g, b) => {
                    let idx = nearest_palette_index(*r, *g, *b, &get_system_default_palette());
                    Ok(player.alloc_datum(Datum::Int(idx as i32)))
                }
            },
            _ => Err(ScriptError::new(format!(
                "Cannot get color property {}",
                prop_name
            ))),
        }
    }

    pub fn set_prop(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        prop: Symbol,
        value: &DatumRef,
    ) -> Result<(), ScriptError> {
        let prop_name = symbols
            .lower(&prop)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        match prop_name {
            "red" => {
                let r = player.get_datum(value).int_value()?;
                let color_ref = player.get_datum_mut(datum).to_color_ref_mut()?;
                match color_ref {
                    ColorRef::Rgb(_, g, b) => {
                        *color_ref = ColorRef::Rgb(r as u8, *g, *b);
                        Ok(())
                    }
                    ColorRef::PaletteIndex(_) => {
                        *color_ref = ColorRef::Rgb(r as u8, 0, 0);
                        Ok(())
                    }
                }
            }
            "green" => {
                let g = player.get_datum(value).int_value()?;
                let color_ref = player.get_datum_mut(datum).to_color_ref_mut()?;
                match color_ref {
                    ColorRef::Rgb(r, _, b) => {
                        *color_ref = ColorRef::Rgb(*r, g as u8, *b);
                        Ok(())
                    }
                    ColorRef::PaletteIndex(_) => {
                        *color_ref = ColorRef::Rgb(0, g as u8, 0);
                        Ok(())
                    }
                }
            }
            "blue" => {
                let b = player.get_datum(value).int_value()?;
                let color_ref = player.get_datum_mut(datum).to_color_ref_mut()?;
                match color_ref {
                    ColorRef::Rgb(r, g, _) => {
                        *color_ref = ColorRef::Rgb(*r, *g, b as u8);
                        Ok(())
                    }
                    ColorRef::PaletteIndex(_) => {
                        *color_ref = ColorRef::Rgb(0, 0, b as u8);
                        Ok(())
                    }
                }
            }
            "colortype" => {
                let symbol = player.get_datum(value).string_value(symbols)?;
                let color_ref = player.get_datum(datum).to_color_ref()?.clone();
                // `the colorType of c = #paletteIndex` — the symbol's display
                // spelling is whichever casing was interned first, so compare
                // case-insensitively as Director does.
                match_ci!(symbol, {
                    "rgb" => {
                        if let ColorRef::PaletteIndex(_) = color_ref {
                            let (r, g, b) = resolve_color_ref(
                                &player.movie.cast_manager.palettes(),
                                &color_ref,
                                &PaletteRef::BuiltIn(get_system_default_palette()),
                                8,
                            );
                            let color_mut = player.get_datum_mut(datum).to_color_ref_mut()?;
                            *color_mut = ColorRef::Rgb(r, g, b);
                        }
                        Ok(())
                    },
                    "paletteIndex" => {
                        if let ColorRef::Rgb(r, g, b) = color_ref {
                            let luminance = (r as u16 * 30 + g as u16 * 59 + b as u16 * 11) / 100;
                            let index = if luminance > 128 { 0u8 } else { 255u8 };
                            let color_mut = player.get_datum_mut(datum).to_color_ref_mut()?;
                            *color_mut = ColorRef::PaletteIndex(index);
                        }
                        Ok(())
                    },
                    _ => Err(ScriptError::new(format!(
                        "Invalid colorType: {}. Expected #rgb or #paletteIndex", symbol
                    )))
                })
            }
            _ => Err(ScriptError::new(format!(
                "Cannot set color property {}",
                symbols.display(&prop).unwrap_or("<foreign symbol>")
            ))),
        }
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
