use crate::{
    director::lingo::datum::Datum,
    player::{
        allocator::ScriptInstanceAllocatorTrait,
        bitmap::bitmap::PaletteRef,
        sprite::ColorRef,
        symbols::symbol_table::SymbolTable,
        DirPlayer, ScriptError, ScriptErrorCode,
    },
};

use super::DatumRef;

#[inline]
fn invalid_reference(message: String) -> ScriptError {
    ScriptError::new_code(ScriptErrorCode::InvalidReference, message)
}

fn symbol_text<'a>(
    symbol: &crate::player::symbols::symbol::Symbol,
    symbols: &'a SymbolTable,
) -> Result<&'a str, ScriptError> {
    symbols
        .display(symbol)
        .map_err(|_| ScriptError::from(crate::player::symbols::symbol::SymbolError::Foreign))
}

pub fn format_concrete_datum(
    datum: &Datum,
    symbols: &SymbolTable,
    player: &DirPlayer,
) -> Result<String, ScriptError> {
    format_concrete_datum_with_depth(datum, symbols, player, 0, usize::MAX)
}

pub fn format_concrete_datum_with_depth(
    datum: &Datum,
    symbols: &SymbolTable,
    player: &DirPlayer,
    depth: usize,
    max_depth: usize,
) -> Result<String, ScriptError> {
    let formatted = match datum {
        Datum::String(s) => format!("\"{s}\""),
        Datum::Int(i) => i.to_string(),
        Datum::Float(f) => format_float_with_precision(*f, player),
        Datum::List(_, items, _) => {
            let formatted_items = items
                .iter()
                .map(|item| format_datum_with_depth(item, symbols, player, depth + 1, max_depth))
                .collect::<Result<Vec<_>, _>>()?;
            format!("[{}]", formatted_items.join(", "))
        }
        Datum::VarRef(_) => "VarRef".to_string(),
        Datum::Void => "Void".to_string(),
        Datum::Symbol(s) => format!("#{}", symbol_text(s, symbols)?),
        Datum::CastLib(n) => format!("castLib({n})"),
        Datum::Stage => "the stage".to_string(),
        Datum::PropList(entries, ..) => {
            if entries.is_empty() {
                return Ok("[:]".to_string());
            }
            let formatted_entries = entries
                .iter()
                .map(|(key, value)| {
                    Ok(format!(
                        "{}: {}",
                        format_datum_with_depth(key, symbols, player, depth + 1, max_depth)?,
                        format_datum_with_depth(value, symbols, player, depth + 1, max_depth)?
                    ))
                })
                .collect::<Result<Vec<_>, ScriptError>>()?;
            format!("[{}]", formatted_entries.join(", "))
        }
        Datum::StringChunk(..) => format!("\"{}\"", datum.string_value(symbols)?),
        Datum::ScriptRef(member_ref) => {
            let script = player
                .movie
                .cast_manager
                .get_script_by_ref(member_ref)
                .ok_or_else(|| invalid_reference(format!("stale script reference {member_ref:?}")))?;
            format!("(script {})", script.name)
        }
        Datum::ScriptInstanceRef(instance_ref) => {
            let instance = player
                .allocator
                .get_script_instance_opt(instance_ref)
                .ok_or_else(|| invalid_reference(format!("stale script instance reference {instance_ref:?}")))?;
            if let Some(script) = player.movie.cast_manager.get_script_by_ref(&instance.script) {
                format!("<offspring {} {} _>", script.name, instance_ref)
            } else {
                format!("<offspring {} {} _>", "(stale)", instance_ref)
            }
        }
        Datum::CastMember(member_ref) => {
            format!("(member {} of castLib {})", member_ref.cast_member, member_ref.cast_lib)
        }
        Datum::SpriteRef(sprite_ref) => format!("(sprite {sprite_ref})"),
        Datum::Rect(vals, flags) => {
            let left = Datum::inline_component_to_datum(vals[0], Datum::inline_is_float(*flags, 0));
            let top = Datum::inline_component_to_datum(vals[1], Datum::inline_is_float(*flags, 1));
            let right = Datum::inline_component_to_datum(vals[2], Datum::inline_is_float(*flags, 2));
            let bottom = Datum::inline_component_to_datum(vals[3], Datum::inline_is_float(*flags, 3));
            format!(
                "rect({}, {}, {}, {})",
                format_numeric_value(&left, symbols, player)?,
                format_numeric_value(&top, symbols, player)?,
                format_numeric_value(&right, symbols, player)?,
                format_numeric_value(&bottom, symbols, player)?
            )
        }
        Datum::Point(vals, flags) => {
            let x = Datum::inline_component_to_datum(vals[0], Datum::inline_is_float(*flags, 0));
            let y = Datum::inline_component_to_datum(vals[1], Datum::inline_is_float(*flags, 1));
            format!(
                "point({}, {})",
                format_numeric_value(&x, symbols, player)?,
                format_numeric_value(&y, symbols, player)?
            )
        }
        Datum::SoundChannel(_) => "<soundChannel>".to_string(),
        Datum::CursorRef(_) => "<cursor>".to_string(),
        Datum::TimeoutRef(name) => format!("timeout(\"{name}\")"),
        Datum::TimeoutFactory => "<timeoutFactory>".to_string(),
        Datum::TimeoutInstance(instance) => format!("timeoutInstance(\"{}\")", instance.name),
        Datum::ColorRef(color_ref) => match color_ref {
            ColorRef::PaletteIndex(index) => format!("color({index})"),
            ColorRef::Rgb(red, green, blue) => format!("rgb({red}, {green}, {blue})"),
        },
        Datum::BitmapRef(bitmap_ref) => {
            let bitmap = player
                .bitmap_manager
                .get_bitmap(*bitmap_ref)
                .ok_or_else(|| invalid_reference(format!("stale bitmap reference {bitmap_ref:?}")))?;
            format!("<bitmap {}x{}x{}>", bitmap.width, bitmap.height, bitmap.bit_depth)
        }
        Datum::PaletteRef(palette_ref) => match palette_ref {
            PaletteRef::BuiltIn(builtin) => format!("{builtin:?}").to_lowercase(),
            PaletteRef::Member(member_ref) => {
                format!("(member {} of castLib {})", member_ref.cast_member, member_ref.cast_lib)
            }
            PaletteRef::Default => "#default".to_string(),
        },
        Datum::Xtra(name) => format!("<Xtra \"{name}\" _ _______>"),
        Datum::XtraInstance(name, instance_id) => format!("<Xtra child \"{name}\" #{instance_id}>"),
        Datum::Matte(..) => "<mask:0000000>".to_string(),
        Datum::Null => "<Null>".to_string(),
        Datum::PlayerRef => "<_player>".to_string(),
        Datum::MovieRef => "<_movie>".to_string(),
        Datum::MouseRef => "<_mouse>".to_string(),
        Datum::XmlRef(id) => format!("<xml:{id}>"),
        Datum::JsObjectRef(_) => "[object Object]".to_string(),
        Datum::MathRef(_) => "<math>".to_string(),
        Datum::Vector(vector) => format!(
            "vector({}, {}, {})",
            format_float_with_precision(vector[0], player),
            format_float_with_precision(vector[1], player),
            format_float_with_precision(vector[2], player),
        ),
        Datum::SoundRef(_) => "<_sound>".to_string(),
        Datum::DateRef(id) => {
            // Director 11.5 Scripting Dictionary, date() (System):
            // _system.date() returns the current date in the system clock,
            // and its own example indexes the result as text
            // (_system.date().char[1..4] = "1/1/"), so a date must render as
            // a DATE STRING, not a type placeholder. AreaZero's startup log
            // printed "Movie started at @ 18:02:17<date>." because we emitted
            // the placeholder into the concatenation.
            //
            // The dictionary notes the format follows the machine's date
            // settings, so defer to the host locale rather than hardcoding a
            // layout. Measured in Director: put _system.date() -> "03.08.2026"
            // - no leading space. The space that separates time from date in
            // a concatenation belongs to _system.time() (Windows short-time
            // keeps an empty AM/PM slot in 24-hour locales); see
            // TypeHandlers::time.
            player.date_objects.get(id)
                .map(|date| js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(date.timestamp_ms as f64))
                    .to_locale_date_string("default", &date_opts())
                    .as_string()
                    .unwrap_or_default())
                .unwrap_or_else(|| "<date>".to_string())
        }
        Datum::Media(_) => "<media>".to_string(),
        Datum::JavaScript(data) => format!("<javascript {} bytes>", data.len()),
        Datum::FlashObjectRef(object_ref) => format!("<Flash object: {}>", object_ref.path),
        Datum::Shockwave3dObjectRef(object_ref) => format!(
            "{}(\"{}\")",
            object_ref.object_type,
            symbol_text(&object_ref.name, symbols)?
        ),
        Datum::Transform3d(matrix) => format!(
            "transform({:.2},{:.2},{:.2},{:.2}, {:.2},{:.2},{:.2},{:.2}, {:.2},{:.2},{:.2},{:.2}, {:.2},{:.2},{:.2},{:.2})",
            matrix[0], matrix[1], matrix[2], matrix[3], matrix[4], matrix[5], matrix[6], matrix[7],
            matrix[8], matrix[9], matrix[10], matrix[11], matrix[12], matrix[13], matrix[14], matrix[15]
        ),
        Datum::HavokObjectRef(object_ref) => format!(
            "{}(\"{}\")",
            object_ref.object_type,
            symbol_text(&object_ref.name, symbols)?
        ),
        Datum::PhysXObjectRef(object_ref) => format!(
            "{}(\"{}\")",
            object_ref.object_type,
            symbol_text(&object_ref.name, symbols)?
        ),
        Datum::VectorVertexRef(member_ref, index) => {
            format!("vertex[{}] of member({}, {})", index + 1, member_ref.cast_member, member_ref.cast_lib)
        }
    };
    Ok(formatted)
}

pub fn datum_to_string_for_concat(
    datum: &Datum,
    symbols: &SymbolTable,
    player: &DirPlayer,
) -> Result<String, ScriptError> {
    match datum {
        Datum::String(value) => Ok(value.clone()),
        Datum::Symbol(symbol) => Ok(symbol_text(symbol, symbols)?.to_owned()),
        Datum::Void | Datum::Null => Ok(String::new()),
        Datum::ColorRef(color_ref) => Ok(match color_ref {
            ColorRef::PaletteIndex(index) => format!("color({index})"),
            ColorRef::Rgb(red, green, blue) => format!("rgb({red}, {green}, {blue})"),
        }),
        Datum::List(..) | Datum::PropList(..) => format_concrete_datum(datum, symbols, player),
        Datum::StringChunk(..) => datum.string_value(symbols),
        _ => format_concrete_datum(datum, symbols, player),
    }
}

pub fn format_datum(
    datum_ref: &DatumRef,
    symbols: &SymbolTable,
    player: &DirPlayer,
) -> Result<String, ScriptError> {
    format_datum_with_depth(datum_ref, symbols, player, 0, usize::MAX)
}

pub fn format_datum_with_depth(
    datum_ref: &DatumRef,
    symbols: &SymbolTable,
    player: &DirPlayer,
    depth: usize,
    max_depth: usize,
) -> Result<String, ScriptError> {
    let datum = match datum_ref {
        DatumRef::Void => &Datum::Void,
        _ => player.allocator.try_get_datum(datum_ref).ok_or_else(|| {
            invalid_reference(format!("invalid datum reference {datum_ref}"))
        })?,
    };
    if depth >= max_depth {
        return Ok(format!("<{datum_ref}>"));
    }
    format_concrete_datum_with_depth(datum, symbols, player, depth, max_depth)
}

pub fn format_float_with_precision(val: f64, player: &DirPlayer) -> String {
    // Normalize negative zero to positive zero
    let val = if val == 0.0 { 0.0 } else { val };

    let fp = player.float_precision as i32;
    
    // Calculate how many characters the decimal notation would take
    let integer_digits = if val.abs() < 1.0 {
        1 // Just "0"
    } else {
        (val.abs().log10().floor() as i32 + 1).max(1)
    };
    
    let decimal_places = if fp > 0 { fp } else { 0 };
    let total_chars = integer_digits + 1 + decimal_places; // digits + '.' + decimals
    
    // Director switches to scientific notation when formatted string >= 18 chars
    if total_chars >= 18 {
        return format!("{:.14e}", val);
    }
    
    // Normal formatting based on floatPrecision
    if fp > 0 {
        let p = fp.min(15) as usize;
        format!("{:.*}", p, val)
    } else if fp == 0 {
        format!("{}", val.round() as i32)
    } else {
        let p = (-fp).min(15);
        let pow = 10f64.powi(p);
        let rounded = (val * pow).round() / pow;
        let s = format!("{:.*}", p as usize, rounded);
        s.trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    }
}

pub fn format_numeric_value(
    datum: &Datum,
    symbols: &SymbolTable,
    player: &DirPlayer,
) -> Result<String, ScriptError> {
    match datum {
        Datum::Int(value) => Ok(value.to_string()),
        Datum::Float(value) => Ok(format_float_with_precision(*value, player)),
        _ => format_concrete_datum(datum, symbols, player),
    }
}

/// Options for Director's system SHORT date: zero-padded day and month with a
/// full year. Measured against Director on this machine: 03.08.2026, where a
/// bare toLocaleDateString gave 3.8.2026. The locale still chooses the field
/// ORDER and separator - only the padding is pinned, matching the dictionary's
/// note that the date format varies, depending on how the date is formatted on
/// the computer.
fn date_opts() -> js_sys::Object {
    let o = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&o, &"day".into(), &"2-digit".into());
    let _ = js_sys::Reflect::set(&o, &"month".into(), &"2-digit".into());
    let _ = js_sys::Reflect::set(&o, &"year".into(), &"numeric".into());
    o
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::player::ownership::OwnerToken;
    use async_std::channel;
    use std::collections::VecDeque;

    fn test_player() -> DirPlayer {
        let (tx, _rx) = channel::unbounded();
        DirPlayer::new_with_owner(tx, OwnerToken::transitional())
    }

    #[test]
    fn formats_authoritative_nested_list_and_proplist_names() {
        let mut player = test_player();
        let mut symbols = SymbolTable::new();
        let key = symbols.intern_authoritative("MiXeDKey");
        let value = player.alloc_datum(Datum::String("value".to_string()));
        let key_ref = player.alloc_datum(Datum::Symbol(key.clone()));
        let nested = player.alloc_datum(Datum::PropList(
            VecDeque::from([(key_ref, value)]),
            false,
        ));
        let list = player.alloc_datum(Datum::List(
            crate::director::lingo::datum::DatumType::List,
            VecDeque::from([nested]),
            false,
        ));

        assert_eq!(
            format_datum(&list, &symbols, &player).unwrap(),
            "[[#MiXeDKey: \"value\"]]"
        );
        assert_eq!(
            format_numeric_value(&Datum::Symbol(key), &symbols, &player).unwrap(),
            "#MiXeDKey"
        );
        assert_eq!(
            format_datum(&DatumRef::Void, &symbols, &player).unwrap(),
            "Void"
        );
    }

    #[test]
    fn rejects_foreign_symbol_even_when_nested() {
        let mut player = test_player();
        let local = SymbolTable::new();
        let mut foreign_table = SymbolTable::new();
        let foreign = foreign_table.intern_authoritative("ForeignName");
        let foreign_ref = player.alloc_datum(Datum::Symbol(foreign));
        let nested = player.alloc_datum(Datum::List(
            crate::director::lingo::datum::DatumType::List,
            VecDeque::from([foreign_ref]),
            false,
        ));
        assert!(format_datum(&nested, &local, &player).is_err());

        let property_key = player.alloc_datum(Datum::Symbol(
            foreign_table.intern_authoritative("ForeignPropertyKey"),
        ));
        let property_value = player.alloc_datum(Datum::String("value".to_string()));
        let properties = player.alloc_datum(Datum::PropList(
            VecDeque::from([(property_key, property_value)]),
            false,
        ));
        assert!(format_datum(&properties, &local, &player).is_err());

        let mut other_player = test_player();
        let foreign_datum_ref = other_player.alloc_datum(Datum::Int(3));
        assert!(format_datum_with_depth(&foreign_datum_ref, &local, &player, 0, 0).is_err());
    }
}
