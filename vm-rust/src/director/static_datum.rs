use std::borrow::Borrow;
use std::collections::VecDeque;
use std::io::Read;

use binary_reader::BinaryReader;
use binary_rw::{BinaryWriter, MemoryStream};
use itertools::Itertools;
use log::warn;

use crate::director::lingo::datum::{Datum, DatumType};
use crate::director::media::reader::MediaReader;
use crate::director::media::writer::MediaWriter;
use crate::player::cast_member::Media;
use crate::player::datum_ref::DatumRef;

use crate::player::allocator::{DatumAllocator, DatumAllocatorTrait};
use crate::player::{DirPlayer, ScriptError};
use crate::player::symbols::symbol::Symbol;
use crate::player::symbols::symbol_table::SymbolTable;

#[derive(Clone, Debug, PartialEq)]
pub enum StaticDatum {
    Int(i32),
    Float(f64),
    String(String),
    Symbol(String),
    List(Vec<StaticDatum>),
    PropList(Vec<(StaticDatum, StaticDatum)>),
    IntPoint(i32, i32),
    IntRect(i32, i32, i32, i32),
    Media(Vec<u8>),
    Void,
}

/// Convert a runtime datum through the caller's player and symbol table.
/// Dynamic symbol spelling and nested references are resolved only while both
/// owner contexts are available; there is no allocator-only fallback.
pub fn static_datum_from_datum_ref(
    player: &DirPlayer,
    symbols: &SymbolTable,
    datum_ref: &DatumRef,
) -> Result<StaticDatum, ScriptError> {
    let datum = match datum_ref {
        DatumRef::Void => return Ok(StaticDatum::Void),
        _ => player.allocator.try_get_datum(datum_ref).ok_or_else(|| {
            ScriptError::new_code(crate::player::ScriptErrorCode::InvalidReference, "foreign or stale datum reference".to_owned())
        })?,
    };
    static_datum_from_datum(player, symbols, datum)
}

pub fn static_datum_from_datum(
    player: &DirPlayer,
    symbols: &SymbolTable,
    datum: &Datum,
) -> Result<StaticDatum, ScriptError> {
    Ok(match datum {
        Datum::Int(i) => StaticDatum::Int(*i),
        Datum::Float(f) => StaticDatum::Float(*f),
        Datum::String(s) => StaticDatum::String(s.clone()),
        Datum::Symbol(s) => StaticDatum::Symbol(
            symbols
                .display(s)
                .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
                .to_owned(),
        ),
        Datum::List(_, items, _) => StaticDatum::List(items.iter().map(|item| static_datum_from_datum_ref(player, symbols, item)).collect::<Result<_, _>>()?),
        Datum::PropList(pairs, _) => StaticDatum::PropList(pairs.iter().map(|(key, value)| Ok((
            static_datum_from_datum_ref(player, symbols, key)?,
            static_datum_from_datum_ref(player, symbols, value)?,
        ))).collect::<Result<_, ScriptError>>()?),
        Datum::Point(vals, _) => StaticDatum::IntPoint(vals[0] as i32, vals[1] as i32),
        Datum::Rect(vals, _) => StaticDatum::IntRect(vals[0] as i32, vals[1] as i32, vals[2] as i32, vals[3] as i32),
        Datum::Media(media) => {
            let mut stream = MemoryStream::new();
            let mut writer = BinaryWriter::new(&mut stream, binary_rw::Endian::Big);
            writer.write_media(media, binary_rw::Endian::Little).unwrap();
            StaticDatum::Media(stream.into())
        }
        _ => StaticDatum::Void,
    })
}

/// Parser-only conversion for Lscr literal data. `LiteralStore` emits only
/// owned scalar values (or Void/JavaScript payloads), so no runtime symbol
/// table is needed at this boundary.
pub(crate) fn static_datum_from_literal(literal: &Datum) -> StaticDatum {
    match literal {
        Datum::Int(i) => StaticDatum::Int(*i),
        Datum::Float(f) => StaticDatum::Float(*f),
        Datum::String(s) => StaticDatum::String(s.clone()),
        Datum::Point(vals, _) => StaticDatum::IntPoint(vals[0] as i32, vals[1] as i32),
        Datum::Rect(vals, _) => StaticDatum::IntRect(vals[0] as i32, vals[1] as i32, vals[2] as i32, vals[3] as i32),
        Datum::Void | Datum::JavaScript(_) => StaticDatum::Void,
        _ => StaticDatum::Void,
    }
}

// Helper function to convert StaticDatum to common types
impl StaticDatum {
    pub fn as_string(&self) -> Option<String> {
        match self {
            StaticDatum::String(s) => Some(s.clone()),
            StaticDatum::Symbol(s) => Some(s.clone()),
            StaticDatum::Int(i) => Some(i.to_string()),
            StaticDatum::Float(f) => Some(f.to_string()),
            _ => None,
        }
    }

    pub fn as_integer(&self) -> Option<i32> {
        match self {
            StaticDatum::Int(i) => Some(*i),
            StaticDatum::String(s) => s.parse().ok(),
            _ => None,
        }
    }

    pub fn as_float(&self) -> Option<f64> {
        match self {
            StaticDatum::Float(f) => Some(*f),
            StaticDatum::Int(i) => Some(*i as f64),
            StaticDatum::String(s) => s.parse().ok(),
            _ => None,
        }
    }

    pub fn as_boolean(&self) -> Option<bool> {
        match self {
            StaticDatum::Int(i) => Some(*i != 0),
            _ => None,
        }
    }
}

pub fn static_datum_to_runtime(
    param: &StaticDatum,
    symbols: &mut SymbolTable,
    allocator: &mut DatumAllocator,
    bitmap_manager: &mut crate::player::bitmap::manager::BitmapManager,
) -> DatumRef {
    match param {
        StaticDatum::String(s) => allocator.alloc_datum(Datum::String(s.clone()), bitmap_manager).unwrap(),
        StaticDatum::Int(i) => allocator.alloc_datum(Datum::Int(*i), bitmap_manager).unwrap(),
        StaticDatum::Float(f) => allocator.alloc_datum(Datum::Float(*f), bitmap_manager).unwrap(),
        StaticDatum::Symbol(s) => allocator
            .alloc_datum(Datum::Symbol(symbols.intern(s)), bitmap_manager)
            .unwrap(),
        StaticDatum::List(items) => {
            let datum_refs: VecDeque<DatumRef> = items
                .iter()
                .map(|item| static_datum_to_runtime(item, symbols, allocator, bitmap_manager))
                .collect();
            allocator
                .alloc_datum(Datum::List(DatumType::List, datum_refs, false), bitmap_manager)
                .unwrap()
        }
        StaticDatum::PropList(items) => {
            let datum_refs: VecDeque<(DatumRef, DatumRef)> = items
                .iter()
                .map(|(key, val)| {
                    let key_ref = static_datum_to_runtime(key, symbols, allocator, bitmap_manager);
                    let val_ref = static_datum_to_runtime(val, symbols, allocator, bitmap_manager);
                    (key_ref, val_ref)
                })
                .collect();
            allocator
                .alloc_datum(Datum::PropList(datum_refs, false), bitmap_manager)
                .unwrap()
        }
        StaticDatum::IntPoint(x, y) => {
            allocator.alloc_datum(Datum::Point([*x as f64, *y as f64], 0), bitmap_manager).unwrap()
        }
        StaticDatum::IntRect(left, top, right, bottom) => {
            allocator.alloc_datum(Datum::Rect([*left as f64, *top as f64, *right as f64, *bottom as f64], 0), bitmap_manager).unwrap()
        }
        StaticDatum::Media(bytes) => {
            let mut reader = BinaryReader::from_u8(bytes);
            reader.set_endian(binary_reader::Endian::Big);
            match reader.read_media() {
                Ok(media) => allocator.alloc_datum(Datum::media(media), bitmap_manager).unwrap(),
                Err(e) => {
                    web_sys::console::warn_1(&format!("Failed to parse media from StaticDatum: {}", e).into());
                    DatumRef::Void
                }
            }
        }
        StaticDatum::Void => DatumRef::Void,
        _ => {
            warn!("⚠️ Unhandled StaticDatum type, using Void");
            DatumRef::Void
        }
    }
}

/// Owner-aware conversion used by canonical session Xtra paths. Symbols are
/// interned through the receiver's authoritative table, including nested
/// list/property values, so the result never carries a process-local or
/// foreign symbol handle across player boundaries.
pub fn static_datum_to_runtime_with_symbols(
    param: &StaticDatum,
    symbols: &mut SymbolTable,
    allocator: &mut DatumAllocator,
    bitmap_manager: &mut crate::player::bitmap::manager::BitmapManager,
) -> DatumRef {
    match param {
        StaticDatum::String(s) => allocator.alloc_datum(Datum::String(s.clone()), bitmap_manager).unwrap(),
        StaticDatum::Int(i) => allocator.alloc_datum(Datum::Int(*i), bitmap_manager).unwrap(),
        StaticDatum::Float(f) => allocator.alloc_datum(Datum::Float(*f), bitmap_manager).unwrap(),
        StaticDatum::Symbol(s) => allocator.alloc_datum(Datum::Symbol(symbols.intern(s)), bitmap_manager).unwrap(),
        StaticDatum::List(items) => {
            let refs = items.iter().map(|item| static_datum_to_runtime_with_symbols(item, symbols, allocator, bitmap_manager)).collect();
            allocator.alloc_datum(Datum::List(DatumType::List, refs, false), bitmap_manager).unwrap()
        }
        StaticDatum::PropList(items) => {
            let refs = items.iter().map(|(key, value)| (
                static_datum_to_runtime_with_symbols(key, symbols, allocator, bitmap_manager),
                static_datum_to_runtime_with_symbols(value, symbols, allocator, bitmap_manager),
            )).collect();
            allocator.alloc_datum(Datum::PropList(refs, false), bitmap_manager).unwrap()
        }
        StaticDatum::IntPoint(x, y) => allocator.alloc_datum(Datum::Point([*x as f64, *y as f64], 0), bitmap_manager).unwrap(),
        StaticDatum::IntRect(left, top, right, bottom) => allocator.alloc_datum(Datum::Rect([*left as f64, *top as f64, *right as f64, *bottom as f64], 0), bitmap_manager).unwrap(),
        StaticDatum::Media(bytes) => {
            let mut reader = BinaryReader::from_u8(bytes);
            reader.set_endian(binary_reader::Endian::Big);
            match reader.read_media() {
                Ok(media) => allocator.alloc_datum(Datum::media(media), bitmap_manager).unwrap(),
                Err(e) => {
                    web_sys::console::warn_1(&format!("Failed to parse media from StaticDatum: {}", e).into());
                    DatumRef::Void
                }
            }
        }
        StaticDatum::Void => DatumRef::Void,
    }
}
