use log::warn;
use std::collections::HashSet;

use crate::director::lingo::datum::Datum;
use crate::director::lingo::datum;
use crate::player::bitmap::manager::INVALID_BITMAP_REF;
use super::{
    allocator::{DatumAllocator, DatumAllocatorTrait},
    handlers::datum_handlers::cast_member_ref::CastMemberRefHandlers,
    symbols::symbol_table::SymbolTable,
    DatumRef, ScriptError, ScriptErrorCode,
};

#[inline]
fn invalid_reference(datum_ref: &DatumRef) -> ScriptError {
    ScriptError::new_code(
        ScriptErrorCode::InvalidReference,
        format!("invalid datum reference {datum_ref}"),
    )
}

fn symbol_text<'a>(
    symbol: &crate::player::symbols::symbol::Symbol,
    symbols: &'a SymbolTable,
) -> Result<&'a str, ScriptError> {
    symbols
        .display(symbol)
        .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign.into())
}

#[inline]
pub(crate) fn validate_direct_symbol_fields(datum: &Datum, symbols: &SymbolTable) -> Result<(), ScriptError> {
    match datum {
        Datum::Symbol(symbol) => { symbol_text(symbol, symbols)?; }
        Datum::Shockwave3dObjectRef(object) => { symbol_text(&object.name, symbols)?; }
        Datum::HavokObjectRef(object) => { symbol_text(&object.name, symbols)?; }
        Datum::PhysXObjectRef(object) => { symbol_text(&object.name, symbols)?; }
        _ => {}
    }
    Ok(())
}

#[inline]
fn validate_comparison_operands(left: &Datum, right: &Datum, symbols: &SymbolTable) -> Result<(), ScriptError> {
    validate_direct_symbol_fields(left, symbols)?;
    validate_direct_symbol_fields(right, symbols)
}

#[inline]
fn checked_datum<'a>(
    allocator: &'a DatumAllocator,
    datum_ref: &DatumRef,
) -> Result<&'a Datum, ScriptError> {
    match datum_ref {
        DatumRef::Void => Ok(&Datum::Void),
        _ => allocator
            .try_get_datum(datum_ref)
            .ok_or_else(|| invalid_reference(datum_ref)),
    }
}

#[inline]
fn seq_equals(left_seq: &[DatumRef], right_seq: &[DatumRef], allocator: &DatumAllocator, symbols: &SymbolTable) -> Result<bool, ScriptError> {
    if left_seq.len() != right_seq.len() {
        return Ok(false);
    }
    for (left_item, right_item) in left_seq.iter().zip(right_seq.iter()) {
        let left_item = checked_datum(allocator, left_item)?;
        let right_item = checked_datum(allocator, right_item)?;
        if !datum_equals(left_item, right_item, allocator, symbols)? {
            return Ok(false);
        }
    }
    Ok(true)
}

pub fn datum_equals(
    left: &Datum,
    right: &Datum,
    allocator: &DatumAllocator,
    symbols: &SymbolTable,
) -> Result<bool, ScriptError> {
    validate_comparison_operands(left, right, symbols)?;
    use Datum::*;
    match (left, right) {
        (Int(i), other) | (other, Int(i)) => Ok(match other {
            Int(other_i) => *i == *other_i,
            Float(f) => (*i as f64) == *f, // TODO: is this correct? Flutter compares ints instead
            String(s) => s.parse::<i32>().ok() == Some(*i), // Handle string-to-int comparison (e.g., "2" should match key 2)
            sc @ StringChunk(..) => sc.string_value(symbols)?.parse::<i32>().ok() == Some(*i),
            Void => *i == 0,
            _ => false,
        }),

        (Float(f), other) | (other, Float(f)) => Ok(match other {
            Float(other_f) => *f == *other_f,
            Void => *f == 0.0,
            _ => false,
        }),

        // String equality: case-insensitive (like Director `=` operator)
        (s @ (String(_) | StringChunk(..)), other) | (other, s @ (String(_) | StringChunk(..))) => Ok({
            let sstr = s.string_value_cow(symbols)?;
            match other {
                String(_) | StringChunk(..) => sstr.eq_ignore_ascii_case(other.string_value_cow(symbols)?.as_ref()), // Case-insensitive comparison for String and StringChunk
                _ => false,
            }
        }),

        (Void | Null, x) | (x, Void | Null) => Ok(match x {
            Void | Null => true,
            VarRef(datum::VarRef::Script(var_ref)) => !var_ref.is_valid(),
            CastMember(member_ref) => !member_ref.is_valid(), // TODO return true if member is empty?
            BitmapRef(b) => *b == INVALID_BITMAP_REF,
            _ => false,
        }),

        (VarRef(a), o) | (o, VarRef(a)) => Ok(match o {
            VarRef(b) => match (a, b) {
                (datum::VarRef::Script(va), datum::VarRef::Script(vb)) => {
                    !va.is_valid() && !vb.is_valid() || // Both invalid = equal
                        CastMemberRefHandlers::get_cast_slot_number(
                            va.cast_lib as u32,
                            va.cast_member as u32,
                        ) == CastMemberRefHandlers::get_cast_slot_number(
                            vb.cast_lib as u32,
                            vb.cast_member as u32,
                        )
                },
                (datum::VarRef::ScriptInstance(va), datum::VarRef::ScriptInstance(vb)) => {
                    **va == **vb
                },
                _ => false
            },
            _ => false
        }),

        (List(_, l, _), other) | (other, List(_, l, _)) => Ok({
            let l_slice: Vec<_> = l.iter().cloned().collect();
            match other {
                List(_, r, _) => { let r_slice: Vec<_> = r.iter().cloned().collect(); seq_equals(&l_slice, &r_slice, allocator, symbols)? },
                Point(vals, flags) => {
                    // Director treats 2-element lists and points interchangeably
                    if l_slice.len() != 2 { false }
                    else {
                        let lx = checked_datum(allocator, &l_slice[0])?;
                        let ly = checked_datum(allocator, &l_slice[1])?;
                        let px = Datum::inline_component_to_datum(vals[0], Datum::inline_is_float(*flags, 0));
                        let py = Datum::inline_component_to_datum(vals[1], Datum::inline_is_float(*flags, 1));
                        datum_equals(lx, &px, allocator, symbols)? && datum_equals(ly, &py, allocator, symbols)?
                    }
                }
                Rect(vals, flags) => {
                    // Director treats 4-element lists and rects interchangeably
                    if l_slice.len() != 4 { false }
                    else {
                        let mut eq = true;
                        for i in 0..4 {
                            let li = checked_datum(allocator, &l_slice[i])?;
                            let ri = Datum::inline_component_to_datum(vals[i], Datum::inline_is_float(*flags, i));
                            if !datum_equals(li, &ri, allocator, symbols)? { eq = false; break; }
                        }
                        eq
                    }
                }
                _ => false
            }
        }),

        (PropList(pairs_a, _), PropList(pairs_b, _)) => {
            // Fast path: same datum in memory (same DatumRef)
            if std::ptr::eq(left, right) {
                return Ok(true);
            }
            // Structural comparison: same keys and values in same order
            if pairs_a.len() != pairs_b.len() {
                return Ok(false);
            }
            for (pair_a, pair_b) in pairs_a.iter().zip(pairs_b.iter()) {
                let key_a = checked_datum(allocator, &pair_a.0)?;
                let key_b = checked_datum(allocator, &pair_b.0)?;
                if !datum_equals(key_a, key_b, allocator, symbols)? {
                    return Ok(false);
                }
                let val_a = checked_datum(allocator, &pair_a.1)?;
                let val_b = checked_datum(allocator, &pair_b.1)?;
                if !datum_equals(val_a, val_b, allocator, symbols)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }

        (Symbol(s), o) | (o, Symbol(s)) => {
            symbol_text(s, symbols)?;
            Ok(match o {
            Symbol(other) => { symbol_text(other, symbols)?; s == other }
            _ => false
            })
        },

        (CastLib(a), o) | (o, CastLib(a)) => Ok(match o {
            CastLib(b) => a == b,
            _ => false
        }),

        (Stage, o) | (o, Stage) => Ok(matches!(o, Stage)),

        (ScriptRef(a), o) | (o, ScriptRef(a)) => Ok(match o {
            ScriptRef(b) => a == b,
            _ => false
        }),

        (ScriptInstanceRef(a), o) | (o, ScriptInstanceRef(a)) => Ok(match o {
            ScriptInstanceRef(b) => **a == **b,
            _ => false
        }),

        (CastMember(a), o) | (o, CastMember(a)) => Ok(match o {
            CastMember(b) => CastMemberRefHandlers::get_cast_slot_number(
                a.cast_lib as u32,
                a.cast_member as u32,
            ) == CastMemberRefHandlers::get_cast_slot_number(
                b.cast_lib as u32,
                b.cast_member as u32,
            ),
            _ => false
        }),

        (SpriteRef(a), o) | (o, SpriteRef(a)) => Ok(match o {
            SpriteRef(b) => a == b,
            _ => false
        }),

        (Rect(a_vals, a_flags), o) | (o, Rect(a_vals, a_flags)) => Ok(match o {
            Rect(b_vals, b_flags) => {
                let mut eq = true;
                for i in 0..4 {
                    let ai = Datum::inline_component_to_datum(a_vals[i], Datum::inline_is_float(*a_flags, i));
                    let bi = Datum::inline_component_to_datum(b_vals[i], Datum::inline_is_float(*b_flags, i));
                    if !datum_equals(&ai, &bi, allocator, symbols)? { eq = false; break; }
                }
                eq
            }
            _ => false
        }),

        (Point(a_vals, a_flags), o) | (o, Point(a_vals, a_flags)) => Ok(match o {
            Point(b_vals, b_flags) => {
                let ax = Datum::inline_component_to_datum(a_vals[0], Datum::inline_is_float(*a_flags, 0));
                let ay = Datum::inline_component_to_datum(a_vals[1], Datum::inline_is_float(*a_flags, 1));
                let bx = Datum::inline_component_to_datum(b_vals[0], Datum::inline_is_float(*b_flags, 0));
                let by = Datum::inline_component_to_datum(b_vals[1], Datum::inline_is_float(*b_flags, 1));
                datum_equals(&ax, &bx, allocator, symbols)? && datum_equals(&ay, &by, allocator, symbols)?
            }
            List(_, list, _) if list.len() == 2 => {
                // Director treats 2-element lists and points interchangeably
                let ax = Datum::inline_component_to_datum(a_vals[0], Datum::inline_is_float(*a_flags, 0));
                let ay = Datum::inline_component_to_datum(a_vals[1], Datum::inline_is_float(*a_flags, 1));
                let list_slice: Vec<_> = list.iter().cloned().collect();
                let lx = checked_datum(allocator, &list_slice[0])?;
                let ly = checked_datum(allocator, &list_slice[1])?;
                datum_equals(&ax, lx, allocator, symbols)? && datum_equals(&ay, ly, allocator, symbols)?
            }
            _ => false
        }),

        (SoundChannel(a), o) | (o, SoundChannel(a)) => Ok(match o {
            SoundChannel(b) => a == b,
            _ => false
        }),

        (CursorRef(a), o) | (o, CursorRef(a)) => Ok(match o {
            // TODO: is equality based on value?
            _ => false
        }),

        (TimeoutRef(a), o) | (o, TimeoutRef(a)) => Ok(match o {
            TimeoutRef(b) => a == b,
            _ => false
        }),

        (TimeoutFactory, o) | (o, TimeoutFactory) => Ok(matches!(o, TimeoutFactory)),

        (TimeoutInstance { .. }, o) | (o, TimeoutInstance { .. }) => Ok(match o {
            // TODO: is equality based on value?
            _ => false
        }),

        (ColorRef(a), o) | (o, ColorRef(a)) => Ok(match o {
            ColorRef(b) => a == b,
            _ => false
        }),

        (BitmapRef(a), o) | (o, BitmapRef(a)) => Ok(match o {
            BitmapRef(b) => a == b,
            _ => false
        }),

        (PaletteRef(a), o) | (o, PaletteRef(a)) => Ok(match o {
            PaletteRef(b) => a == b,
            _ => false
        }),

        (SoundRef(a), o) | (o, SoundRef(a)) => Ok(match o {
            SoundRef(b) => a == b,
            _ => false
        }),

        (Xtra(a), o) | (o, Xtra(a)) => Ok(match o {
            Xtra(b) => a == b,
            _ => false
        }),

        (XtraInstance(a, ai), o) | (o, XtraInstance(a, ai)) => Ok(match o {
            XtraInstance(b, bi) => a == b && ai == bi,
            _ => false
        }),

        (Matte(a), o) | (o, Matte(a)) => Ok(match o {
            Matte(b) => a == b,
            _ => false
        }),

        (PlayerRef, o) | (o, PlayerRef) => Ok(matches!(o, PlayerRef)),

        (MovieRef, o) | (o, MovieRef) => Ok(matches!(o, MovieRef)),

        (MouseRef, o) | (o, MouseRef) => Ok(matches!(o, MouseRef)),

        (XmlRef(a), o) | (o, XmlRef(a)) => Ok(match o {
            XmlRef(b) => a == b,
            _ => false
        }),

        (DateRef(a), o) | (o, DateRef(a)) => Ok(match o {
            DateRef(b) => a == b,
            _ => false
        }),

        (FlashObjectRef(a), o) | (o, FlashObjectRef(a)) => Ok(match o {
            // Two AS object refs are equal iff they point to the same path.
            // Without this case, the catch-all at the bottom returns false even
            // for identical refs, which makes any prop list containing AS object
            // values (e.g. Coke Studios' friend list with #lastAccess Date refs)
            // fail deep equality against its own .duplicate() — causing every
            // friendslist exitFrame to redraw the whole list during scrolling.
            FlashObjectRef(b) => a.path == b.path
                && a.cast_lib == b.cast_lib
                && a.cast_member == b.cast_member,
            _ => false
        }),

        (MathRef(a), o) | (o, MathRef(a)) => Ok(match o {
            MathRef(b) => a == b,
            _ => false
        }),

        // Handles are de-duplicated by object identity when they're minted,
        // so equal ids mean the same JS object
        (JsObjectRef(a), o) | (o, JsObjectRef(a)) => Ok(match o {
            JsObjectRef(b) => a == b,
            _ => false
        }),

        (Vector(v), o) | (o, Vector(v)) => Ok(match o {
            Vector(other_v) => v == other_v,
            _ => false
        }),

        // Two PhysX rigid-body / joint / terrain refs are the same Director
        // object when they point to the same (cast_lib, cast_member, id).
        // Without this, `collisionreport.objectA = Vehicle.Physics` in the
        // registered #collisionCallback always returns false (the catch-all
        // below returns false), so OnGround never flips and bodies don't
        // come to rest on the ground.
        (PhysXObjectRef(a), o) | (o, PhysXObjectRef(a)) => Ok(match o {
            PhysXObjectRef(b) => a.cast_lib == b.cast_lib
                && a.cast_member == b.cast_member
                && a.id == b.id,
            _ => false
        }),

        // Same treatment for Havok object refs — LEGO Supersonic's
        // collision callbacks rely on this equality.
        (HavokObjectRef(a), o) | (o, HavokObjectRef(a)) => Ok(match o {
            HavokObjectRef(b) => { symbol_text(&a.name, symbols)?; symbol_text(&b.name, symbols)?; a.cast_lib == b.cast_lib
                && a.cast_member == b.cast_member
                && a.name == b.name },
            _ => false
        }),

        // Two W3D scene-object refs (model/group/light/camera/collision/...)
        // are the same Director object when they point to the same member,
        // object type and (case-insensitive) name. Without this the catch-all
        // returns false and `collisionData.modelA = s.model("ft")` — the heart
        // of every native #collision callback — never matches.
        (Shockwave3dObjectRef(a), o) | (o, Shockwave3dObjectRef(a)) => Ok(match o {
            Shockwave3dObjectRef(b) => { symbol_text(&a.name, symbols)?; symbol_text(&b.name, symbols)?; a.cast_lib == b.cast_lib
                && a.cast_member == b.cast_member
                && a.object_type == b.object_type
                && symbol_text(&a.name, symbols)?.eq_ignore_ascii_case(symbol_text(&b.name, symbols)? ) },
            _ => false
        }),

        (Media(a), o) | (o, Media(a)) => Ok(match o {
            // TODO: is equality based on value?
            _ => false
        }),

        (JavaScript(a), o) | (o, JavaScript(a)) => Ok(match o {
            JavaScript(b) => a == b,
            _ => false
        }),

        _ => {
            warn!(
                "datum_equals not supported for types: {} and {}",
                left.type_str(),
                right.type_str()
            );
            Ok(false)
        }
    }
}

/// List-membership equality (`getPos`, `getOne`, `findPos`).
///
/// Director's `=` operator is strict — `#foo = "foo"` returns FALSE — but the
/// list-membership lookup family matches by text content across the
/// Symbol/String boundary. Verified with `put getPos([#foo, #bar], "foo")`
/// returning 1 in Director 11.5. Scripts in the wild (e.g. Trick or Treat
/// Beat's `getPos(gSingleTileObjNames, member.name)`) rely on this looser
/// rule even though general `=` would not match.
pub fn datum_equals_member(
    left: &Datum,
    right: &Datum,
    allocator: &DatumAllocator,
    symbols: &SymbolTable,
) -> Result<bool, ScriptError> {
    validate_comparison_operands(left, right, symbols)?;
    use Datum::*;
    let symbol_string_match = match (left, right) {
        (Symbol(sym), other @ (String(_) | StringChunk(..)))
        | (other @ (String(_) | StringChunk(..)), Symbol(sym)) => {
            Some(symbol_text(sym, symbols)?.eq_ignore_ascii_case(other.string_value_cow(symbols)?.as_ref()))
        }
        _ => None,
    };
    if let Some(matched) = symbol_string_match {
        return Ok(matched);
    }
    datum_equals(left, right, allocator, symbols)
}

#[allow(dead_code)]
/// Order a string against a number.
///
/// Director reads the string as a number when it can — `"10" > 9` is TRUE, so
/// this cannot be a plain text comparison. When the string is not numeric it
/// falls back to comparing text, with the number rendered as its string form;
/// `"none" > 0` is then TRUE because 'n' sorts after '0'.
///
/// Both halves are load-bearing. Symbols compare as their names, so this is
/// also the rule for `symbol > number`, and Merlin's Revenge depends on it:
///
///   squadNum = teamInfo.squadNum          -- #none when the object has no slot
///   objs = p.teams[teamNum][objectType]   -- []
///   numObjs = objs.count                  -- 0
///   if not (squadNum > numObjs) then
///     objToDel = objs[squadNum]           -- only reached if #none <= 0
///
/// Treating the non-numeric string as 0 made `#none > 0` false, so the guard
/// let through `objs[#none]` on an empty list and every character teardown
/// (leaveTeam) died with an index error.
fn string_number_ordering(text: &str, number: f64, number_text: &str) -> std::cmp::Ordering {
    if let Ok(value) = text.trim().parse::<f64>() {
        return value
            .partial_cmp(&number)
            .unwrap_or(std::cmp::Ordering::Equal);
    }
    text.to_ascii_lowercase()
        .cmp(&number_text.to_ascii_lowercase())
}

pub fn datum_greater_than(left: &Datum, right: &Datum, allocator: &DatumAllocator, symbols: &SymbolTable) -> Result<bool, ScriptError> {
    validate_comparison_operands(left, right, symbols)?;
    // See `datum_less_than`: a string chunk compares by its resolved text, and
    // a sprite reference compares by its sprite (channel) number.
    if let Datum::StringChunk(_, _, s) = left {
        return datum_greater_than(&Datum::String(s.clone()), right, allocator, symbols);
    }
    if let Datum::StringChunk(_, _, s) = right {
        return datum_greater_than(left, &Datum::String(s.clone()), allocator, symbols);
    }
    if let Datum::SpriteRef(n) = left {
        return datum_greater_than(&Datum::Int(*n as i32), right, allocator, symbols);
    }
    if let Datum::SpriteRef(n) = right {
        return datum_greater_than(left, &Datum::Int(*n as i32), allocator, symbols);
    }
    // A symbol compares as its string name (see `datum_less_than`).
    if let Datum::Symbol(s) = left {
        return datum_greater_than(&Datum::String(symbol_text(s, symbols)?.to_owned()), right, allocator, symbols);
    }
    if let Datum::Symbol(s) = right {
        return datum_greater_than(left, &Datum::String(symbol_text(s, symbols)?.to_owned()), allocator, symbols);
    }
    match (left, right) {
        // Int comparisons
        (Datum::Int(left), Datum::Int(right)) => Ok(*left > *right),
        (Datum::Int(left), Datum::Float(right)) => Ok((*left as f64) > *right),
        (Datum::Int(left), Datum::Void) => Ok(*left > 0),
        (Datum::Int(left), Datum::String(right)) => Ok(string_number_ordering(
            right,
            *left as f64,
            &left.to_string(),
        )
        .is_lt()),

        // Float comparisons
        (Datum::Float(left), Datum::Int(right)) => Ok(*left > (*right as f64)),
        (Datum::Float(left), Datum::Float(right)) => Ok(*left > *right),
        (Datum::Float(left), Datum::Void) => Ok(*left > 0.0),
        (Datum::Float(left), Datum::String(right)) => {
            Ok(string_number_ordering(right, *left, &left.to_string()).is_lt())
        }
        
        // Void comparisons - Void is never > any number
        (Datum::Void, Datum::Int(_)) => Ok(false),
        (Datum::Void, Datum::Float(_)) => Ok(false),
        
        // String vs number — see `string_number_ordering`.
        (Datum::String(left), Datum::Int(right)) => Ok(string_number_ordering(
            left,
            *right as f64,
            &right.to_string(),
        )
        .is_gt()),
        (Datum::String(left), Datum::Float(right)) => {
            Ok(string_number_ordering(left, *right, &right.to_string()).is_gt())
        }

        // Point comparisons
        (Datum::Point(left_vals, _), Datum::Point(right_vals, _)) => {
            let left_x = left_vals[0] as i32;
            let left_y = left_vals[1] as i32;
            let right_x = right_vals[0] as i32;
            let right_y = right_vals[1] as i32;
            Ok(left_x > right_x && left_y > right_y)
        }

        // Point vs scalar: Director compares the point against the scalar
        // component-wise; the result is true when ANY component satisfies the
        // comparison. Summer Resort's room-scroll clamp relies on this — it
        // tests an axis-aligned delta `point(0,16) > 0` / `point(0,-16) < 0`
        // (one component is 0, the other ±16) to decide the scroll direction
        // and clamp the player to the room edge. Without this the clamp never
        // fired, the player over-scrolled past the screen bottom, and the next
        // move bounced straight back into the previous room.
        (Datum::Point(vals, _), Datum::Int(n)) => {
            Ok((vals[0] as i32) > *n || (vals[1] as i32) > *n)
        }
        (Datum::Int(n), Datum::Point(vals, _)) => {
            Ok(*n > (vals[0] as i32) || *n > (vals[1] as i32))
        }

        // Linear list comparison — element-wise, mirroring `datum_less_than`
        // (and the 11.5 dictionary's rect/point-as-list rule). True only if
        // every corresponding element of the left is > the right's.
        (Datum::List(_, left_items, _), Datum::List(_, right_items, _)) => {
            if left_items.is_empty() || right_items.is_empty() {
                return Ok(false);
            }
            for (l, r) in left_items.iter().zip(right_items.iter()) {
                let ld = checked_datum(allocator, l)?;
                let rd = checked_datum(allocator, r)?;
                if !datum_greater_than(ld, rd, allocator, symbols)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }

        // Script instances compare by allocation id (see `datum_less_than`).
        (Datum::ScriptInstanceRef(l), Datum::ScriptInstanceRef(r)) => Ok(l.id() > r.id()),

        // Two strings compare lexicographically, case-insensitively — the mirror of
        // `datum_less_than`. (Symbols are pre-converted to strings above.) Director
        // uses this for text/version checks like `GrooveVersion() >= "1.7"`.
        (Datum::String(left), Datum::String(right)) =>
            Ok(left.to_ascii_lowercase() > right.to_ascii_lowercase()),

        // Catch-all
        _ => {
            warn!(
                "datum_greater_than not supported for types: {} and {}",
                left.type_str(),
                right.type_str()
            );
            Ok(false)
        }
    }
}

/// Case-insensitive lexicographic `<`, without allocating.
///
/// Byte-for-byte equivalent to `a.to_ascii_lowercase() < b.to_ascii_lowercase()`:
/// `str`'s ordering is bytewise over UTF-8, and `to_ascii_lowercase` only ever
/// remaps `A-Z` (single-byte, and never a UTF-8 continuation byte), so folding
/// per byte during the walk gives the same answer without the two temporaries.
#[inline]
fn str_lt_ci(a: &str, b: &str) -> bool {
    a.as_bytes()
        .iter()
        .map(u8::to_ascii_lowercase)
        .cmp(b.as_bytes().iter().map(u8::to_ascii_lowercase))
        .is_lt()
}

pub fn datum_less_than(left: &Datum, right: &Datum, allocator: &DatumAllocator, symbols: &SymbolTable) -> Result<bool, ScriptError> {
    validate_comparison_operands(left, right, symbols)?;
    // A string chunk (`char/word/item/line N of x`) evaluates to a plain
    // string — compare by its resolved text. Without this, `char 1 of x < "m"`
    // (and any chunk poll in a per-frame loop) falls through to the catch-all,
    // returning a wrong result AND warning every frame.
    if let Datum::StringChunk(_, _, s) = left {
        return datum_less_than(&Datum::String(s.clone()), right, allocator, symbols);
    }
    if let Datum::StringChunk(_, _, s) = right {
        return datum_less_than(left, &Datum::String(s.clone()), allocator, symbols);
    }
    // A sprite reference compares by its sprite (channel) number, so a movie
    // that sorts/compares `sprite(a) < sprite(b)` works instead of hitting the
    // catch-all (wrong result + per-frame warn).
    if let Datum::SpriteRef(n) = left {
        return datum_less_than(&Datum::Int(*n as i32), right, allocator, symbols);
    }
    if let Datum::SpriteRef(n) = right {
        return datum_less_than(left, &Datum::Int(*n as i32), allocator, symbols);
    }
    // Symbol/string orderings are the hot case: `PropListUtils::get_key_index`
    // runs one of these per binary-search probe (and per entry on its linear
    // fallback), so a prop-list read did O(log n) comparisons. Handle the three
    // text-vs-text pairs here WITHOUT allocating.
    //
    // These used to fall into the blanket symbol->String conversion below and
    // then into the `to_ascii_lowercase() < to_ascii_lowercase()` arms further
    // down, costing FOUR heap allocations per comparison (two to stringify the
    // symbols, two to lowercase them) — the dedicated Symbol arms in the match
    // were dead code because the conversion always fired first.
    match (left, right) {
        (Datum::Symbol(l), Datum::Symbol(r)) => {
            return Ok(str_lt_ci(symbol_text(l, symbols)?, symbol_text(r, symbols)?))
        }
        (Datum::Symbol(l), Datum::String(r)) => return Ok(str_lt_ci(symbol_text(l, symbols)?, r)),
        (Datum::String(l), Datum::Symbol(r)) => return Ok(str_lt_ci(l, symbol_text(r, symbols)?)),
        _ => {}
    }
    // A symbol compares as its string name (Director treats `#foo` like "foo" in
    // ordered comparisons), so route symbols through the string logic below. This
    // covers int-vs-symbol and the other mixed pairs without dedicated arms.
    if let Datum::Symbol(s) = left {
        return datum_less_than(&Datum::String(symbol_text(s, symbols)?.to_owned()), right, allocator, symbols);
    }
    if let Datum::Symbol(s) = right {
        return datum_less_than(left, &Datum::String(symbol_text(s, symbols)?.to_owned()), allocator, symbols);
    }
    match (left, right) {
        // Int comparisons
        (Datum::Int(left), Datum::Int(right)) => Ok(*left < *right),
        (Datum::Int(left), Datum::Float(right)) => Ok((*left as f64) < *right),
        (Datum::Int(left), Datum::Void) => Ok(*left < 0),
        (Datum::Int(left), Datum::String(right)) => Ok(string_number_ordering(
            right,
            *left as f64,
            &left.to_string(),
        )
        .is_gt()),

        // Float comparisons
        (Datum::Float(left), Datum::Int(right)) => Ok(*left < (*right as f64)),
        (Datum::Float(left), Datum::Float(right)) => Ok(*left < *right),
        (Datum::Float(left), Datum::Void) => Ok(*left < 0.0),
        (Datum::Float(left), Datum::String(right)) => {
            Ok(string_number_ordering(right, *left, &left.to_string()).is_gt())
        }
        
        // Void comparisons - Void is always < any number
        (Datum::Void, Datum::Int(_)) => Ok(true),
        (Datum::Void, Datum::Float(_)) => Ok(true),
        
        // Point comparisons
        (Datum::Point(left_vals, _), Datum::Point(right_vals, _)) => {
            let left_x = left_vals[0] as i32;
            let left_y = left_vals[1] as i32;
            let right_x = right_vals[0] as i32;
            let right_y = right_vals[1] as i32;
            Ok(left_x < right_x && left_y < right_y)
        }

        // Point vs scalar — see the note in `datum_greater_than`. Any component
        // satisfying the comparison makes it true (axis-aligned scroll deltas).
        (Datum::Point(vals, _), Datum::Int(n)) => {
            Ok((vals[0] as i32) < *n || (vals[1] as i32) < *n)
        }
        (Datum::Int(n), Datum::Point(vals, _)) => {
            Ok(*n < (vals[0] as i32) || *n < (vals[1] as i32))
        }

        // String vs number — see `string_number_ordering`.
        (Datum::String(left), Datum::Int(right)) => Ok(string_number_ordering(
            left,
            *right as f64,
            &right.to_string(),
        )
        .is_lt()),
        (Datum::String(left), Datum::Float(right)) => {
            Ok(string_number_ordering(left, *right, &right.to_string()).is_lt())
        }

        // String / Symbol comparisons — Director compares case-insensitively
        // lexicographically. Without this, any `.add()` to a sorted list of
        // strings (e.g. CS FurnitureItem draw-order tags "a","b","c","d") would
        // always return 0 from find_index_to_add and silently prepend every
        // item, breaking sprite-pool allocation order.
        // (The Symbol pairings are handled allocation-free at the top of this
        // function, before the symbol->String conversion.)
        (Datum::String(left), Datum::String(right)) => Ok(str_lt_ci(left, right)),

        // String comparisons
        (Datum::String(..), Datum::String(..)) => Ok(false),

        // PropList comparisons - Director compares property lists by their first value.
        // This is essential for sorted lists used as priority queues (e.g. A* pathfinding).
        (Datum::PropList(left_pairs, ..), Datum::PropList(right_pairs, ..)) => {
            if let (Some((_, left_val)), Some((_, right_val))) = (left_pairs.front(), right_pairs.front()) {
                let left_datum = checked_datum(allocator, left_val)?;
                let right_datum = checked_datum(allocator, right_val)?;
                datum_less_than(left_datum, right_datum, allocator, symbols)
            } else {
                Ok(false)
            }
        }

        // Linear list comparison. Per the 11.5 dictionary `<` entry, rects/points
        // (and by extension lists) compare "with each element of the first list
        // compared to the corresponding element of the second list" — the same
        // all-components rule the Point arm above uses. True only if every
        // corresponding element of the left is < the right's.
        (Datum::List(_, left_items, _), Datum::List(_, right_items, _)) => {
            if left_items.is_empty() || right_items.is_empty() {
                return Ok(false);
            }
            for (l, r) in left_items.iter().zip(right_items.iter()) {
                let ld = checked_datum(allocator, l)?;
                let rd = checked_datum(allocator, r)?;
                if !datum_less_than(ld, rd, allocator, symbols)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }

        // Script instances have no meaningful ordering in Director, but a movie
        // that sorts or compares them needs a stable result — compare by their
        // allocation id.
        (Datum::ScriptInstanceRef(l), Datum::ScriptInstanceRef(r)) => Ok(l.id() < r.id()),

        // Catch-all
        _ => {
            warn!(
                "datum_less_than not supported for types: {} and {}",
                left.type_str(),
                right.type_str()
            );
            Ok(false)
        }
    }
}

pub fn datum_is_zero(
    datum: &Datum,
    datums: &DatumAllocator,
    symbols: &SymbolTable,
) -> Result<bool, ScriptError> {
    validate_direct_symbol_fields(datum, symbols)?;
    Ok(match datum {
        Datum::Int(value) => *value == 0,
        Datum::Float(value) => *value == 0.0,
        Datum::Void => true,
        Datum::ScriptInstanceRef(_) => false,
        Datum::Null => true,
        Datum::Point(vals, _) => {
            vals[0] as i32 == 0 && vals[1] as i32 == 0
        }
        Datum::Rect(vals, _) => {
            vals[0] as i32 == 0 && vals[1] as i32 == 0 && vals[2] as i32 == 0 && vals[3] as i32 == 0
        }
        _ => {
            warn!("datum_is_zero not supported for type: {}", datum.type_str());
            datum.int_value()? == 0
        }
    })
}

pub fn sort_datums(
    datums: &Vec<DatumRef>,
    allocator: &DatumAllocator,
    symbols: &SymbolTable,
) -> Result<Vec<DatumRef>, ScriptError> {
    let mut visited = HashSet::new();
    for datum_ref in datums {
        validate_reachable_symbols(datum_ref, allocator, symbols, &mut visited)?;
    }
    let mut sorted_list = datums.clone();
    sorted_list.sort_by(|a, b| {
        let left = allocator.get_datum(a);
        let right = allocator.get_datum(b);

        if datum_equals(left, right, allocator, symbols).unwrap() {
            return std::cmp::Ordering::Equal;
        } else if datum_less_than(left, right, allocator, symbols).unwrap() {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Greater
        }
    });
    Ok(sorted_list)
}

/// Validate only symbol-bearing values reachable by a sort comparison.
///
/// Sorting still uses the existing stable `sort_by` comparator. This O(reachable
/// graph) preflight is deliberately separate so a foreign symbol produces an
/// error before the comparator runs, rather than being converted into a
/// fabricated `Ordering`. A visited set makes cyclic list/property-list data
/// terminate; unrelated script-instance graphs remain opaque as before.
pub(crate) fn validate_reachable_symbols(
    datum_ref: &DatumRef,
    allocator: &DatumAllocator,
    symbols: &SymbolTable,
    visited: &mut HashSet<usize>,
) -> Result<(), ScriptError> {
    let Some(datum) = (match datum_ref {
        DatumRef::Void => return Ok(()),
        _ => allocator.try_get_datum(datum_ref),
    }) else {
        return Err(invalid_reference(datum_ref));
    };
    if !visited.insert(datum_ref.unwrap()) {
        return Ok(());
    }
    match datum {
        Datum::Symbol(symbol) => {
            symbol_text(symbol, symbols)?;
        }
        Datum::Shockwave3dObjectRef(object) => {
            symbol_text(&object.name, symbols)?;
        }
        Datum::HavokObjectRef(object) => {
            symbol_text(&object.name, symbols)?;
        }
        Datum::PhysXObjectRef(object) => {
            symbol_text(&object.name, symbols)?;
        }
        Datum::List(_, items, _) => {
            for item in items {
                validate_reachable_symbols(item, allocator, symbols, visited)?;
            }
        }
        Datum::PropList(entries, _) => {
            for (key, value) in entries {
                validate_reachable_symbols(key, allocator, symbols, visited)?;
                validate_reachable_symbols(value, allocator, symbols, visited)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn datum_to_f64(datum: &Datum) -> Result<f64, ScriptError> {
    match datum {
        Datum::Int(i) => Ok(*i as f64),
        Datum::Float(f) => Ok(*f),
        _ => datum.float_value()
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::director::lingo::datum::{
        DatumType, HavokObjectRef, Shockwave3dObjectRef,
    };
    use crate::player::ownership::OwnerToken;
    use crate::player::symbols::{builtin::BuiltInSymbol, symbol_table::SymbolTable};
    use async_std::channel;
    use std::collections::VecDeque;

    fn test_player() -> crate::player::DirPlayer {
        let (tx, _rx) = channel::unbounded();
        crate::player::DirPlayer::new_with_owner(tx, OwnerToken::transitional())
    }

    #[test]
    fn foreign_symbols_are_rejected_by_equality_and_ordering() {
        let allocator = DatumAllocator::default();
        let mut local = SymbolTable::new();
        let mut foreign_table = SymbolTable::new();
        let foreign = foreign_table.intern_authoritative("ForeignCompare");
        let local_symbol = local.intern_authoritative("ForeignCompare");

        assert!(datum_equals(
            &Datum::Symbol(foreign.clone()),
            &Datum::Symbol(local_symbol.clone()),
            &allocator,
            &local,
        ).is_err());
        assert!(datum_equals(
            &Datum::Symbol(foreign.clone()),
            &Datum::Int(0),
            &allocator,
            &local,
        ).is_err());
        assert!(datum_less_than(
            &Datum::Symbol(foreign.clone()),
            &Datum::String("z".to_string()),
            &allocator,
            &local,
        ).is_err());
        assert!(datum_equals_member(
            &Datum::Symbol(foreign),
            &Datum::String("ForeignCompare".to_string()),
            &allocator,
            &local,
        ).is_err());
        assert!(datum_is_zero(&Datum::Symbol(local_symbol), &allocator, &local).unwrap());
        assert!(datum_is_zero(&Datum::Symbol(foreign_table.intern("foreignZero")), &allocator, &local).is_err());
    }

    #[test]
    fn foreign_3d_names_are_rejected() {
        let allocator = DatumAllocator::default();
        let local = SymbolTable::new();
        let mut foreign_table = SymbolTable::new();
        let foreign_name = foreign_table.intern("foreignModel");
        let left = Datum::Shockwave3dObjectRef(Shockwave3dObjectRef {
            cast_lib: 1,
            cast_member: 2,
            object_type: BuiltInSymbol::Model,
            name: foreign_name.clone(),
        });
        let right = Datum::Shockwave3dObjectRef(Shockwave3dObjectRef {
            cast_lib: 1,
            cast_member: 2,
            object_type: BuiltInSymbol::Model,
            name: foreign_name,
        });
        assert!(datum_equals(&left, &right, &allocator, &local).is_err());

        let havok = Datum::HavokObjectRef(HavokObjectRef {
            cast_lib: 1,
            cast_member: 2,
            object_type: BuiltInSymbol::RigidBody,
            name: foreign_table.intern("foreignBody"),
        });
        assert!(datum_equals(&havok, &havok, &allocator, &local).is_err());
    }

    #[test]
    fn nested_comparisons_check_owned_references_and_preserve_void() {
        let mut player = test_player();
        let mut foreign_player = test_player();
        let symbols = SymbolTable::new();
        let local_value = player.alloc_datum(Datum::Int(1));
        let local_two = player.alloc_datum(Datum::Int(2));
        let foreign_value = foreign_player.alloc_datum(Datum::Int(1));
        let foreign_later_equal = foreign_player.alloc_datum(Datum::Int(2));
        let foreign_later_order = foreign_player.alloc_datum(Datum::Int(2));
        let valid_list = player.alloc_datum(Datum::List(
            DatumType::List,
            VecDeque::from([local_value.clone()]),
            false,
        ));
        let foreign_child_list = player.alloc_datum(Datum::List(
            DatumType::List,
            VecDeque::from([foreign_value]),
            false,
        ));
        let early_equal_mismatch = player.alloc_datum(Datum::List(
            DatumType::List,
            VecDeque::from([local_value.clone(), foreign_later_equal]),
            false,
        ));
        let equal_mismatch_target = player.alloc_datum(Datum::List(
            DatumType::List,
            VecDeque::from([local_two.clone(), local_value.clone()]),
            false,
        ));
        let early_order_mismatch = player.alloc_datum(Datum::List(
            DatumType::List,
            VecDeque::from([local_two.clone(), foreign_later_order]),
            false,
        ));
        let order_mismatch_target = player.alloc_datum(Datum::List(
            DatumType::List,
            VecDeque::from([local_value.clone(), local_two.clone()]),
            false,
        ));

        assert!(datum_equals(
            player.get_datum(&foreign_child_list),
            player.get_datum(&valid_list),
            &player.allocator,
            &symbols,
        ).is_err());
        assert_eq!(datum_equals(
            player.get_datum(&early_equal_mismatch),
            player.get_datum(&equal_mismatch_target),
            &player.allocator,
            &symbols,
        ).unwrap(), false);
        assert_eq!(datum_less_than(
            player.get_datum(&early_order_mismatch),
            player.get_datum(&order_mismatch_target),
            &player.allocator,
            &symbols,
        ).unwrap(), false);
        assert!(datum_greater_than(
            player.get_datum(&foreign_child_list),
            player.get_datum(&valid_list),
            &player.allocator,
            &symbols,
        ).is_err());
        assert!(datum_less_than(
            player.get_datum(&foreign_child_list),
            player.get_datum(&valid_list),
            &player.allocator,
            &symbols,
        ).is_err());

        let void_list = player.alloc_datum(Datum::List(
            DatumType::List,
            VecDeque::from([DatumRef::Void]),
            false,
        ));
        let int_list = player.alloc_datum(Datum::List(
            DatumType::List,
            VecDeque::from([local_value]),
            false,
        ));
        assert!(datum_equals(
            player.get_datum(&void_list),
            player.get_datum(&void_list),
            &player.allocator,
            &symbols,
        ).unwrap());
        assert!(datum_less_than(
            player.get_datum(&void_list),
            player.get_datum(&int_list),
            &player.allocator,
            &symbols,
        ).unwrap());
    }

    #[test]
    fn prop_list_comparison_checks_keys_and_values_but_short_circuits() {
        let mut player = test_player();
        let mut foreign_player = test_player();
        let symbols = SymbolTable::new();
        let key = player.alloc_datum(Datum::Int(1));
        let other_key = player.alloc_datum(Datum::Int(2));
        let value = player.alloc_datum(Datum::Int(3));
        let other_value = player.alloc_datum(Datum::Int(4));
        let foreign_key = foreign_player.alloc_datum(Datum::Int(1));
        let foreign_value = foreign_player.alloc_datum(Datum::Int(5));
        let foreign_later_key = foreign_player.alloc_datum(Datum::Int(6));
        let foreign_later_value = foreign_player.alloc_datum(Datum::Int(7));
        let later_foreign_key = player.alloc_datum(Datum::Int(8));
        let later_foreign_value = player.alloc_datum(Datum::Int(9));

        let foreign_key_list = player.alloc_datum(Datum::PropList(
            VecDeque::from([(foreign_key, value.clone())]),
            false,
        ));
        let valid_key_list = player.alloc_datum(Datum::PropList(
            VecDeque::from([(key.clone(), other_value.clone())]),
            false,
        ));
        assert!(datum_equals(
            player.get_datum(&foreign_key_list),
            player.get_datum(&valid_key_list),
            &player.allocator,
            &symbols,
        ).is_err());

        let foreign_value_list = player.alloc_datum(Datum::PropList(
            VecDeque::from([(key.clone(), foreign_value)]),
            false,
        ));
        let valid_value_list = player.alloc_datum(Datum::PropList(
            VecDeque::from([(key.clone(), value.clone())]),
            false,
        ));
        assert!(datum_equals(
            player.get_datum(&foreign_value_list),
            player.get_datum(&valid_value_list),
            &player.allocator,
            &symbols,
        ).is_err());
        assert!(datum_less_than(
            player.get_datum(&foreign_value_list),
            player.get_datum(&valid_value_list),
            &player.allocator,
            &symbols,
        ).is_err());

        let first_mismatch = player.alloc_datum(Datum::PropList(
            VecDeque::from([
                (key, value),
                (foreign_later_key, foreign_later_value),
            ]),
            false,
        ));
        let later_foreign = player.alloc_datum(Datum::PropList(
            VecDeque::from([
                (other_key, other_value),
                (later_foreign_key, later_foreign_value),
            ]),
            false,
        ));
        assert_eq!(datum_equals(
            player.get_datum(&first_mismatch),
            player.get_datum(&later_foreign),
            &player.allocator,
            &symbols,
        ).unwrap(), false);
    }

    #[test]
    fn sort_preflights_foreign_symbols_and_preserves_valid_stability() {
        let mut player = test_player();
        let symbols = SymbolTable::new();
        let first_equal = player.alloc_datum(Datum::String("a".to_string()));
        let second_equal = player.alloc_datum(Datum::String("A".to_string()));
        let greater = player.alloc_datum(Datum::String("b".to_string()));
        let sorted = sort_datums(
            &vec![greater, first_equal.clone(), second_equal.clone()],
            &player.allocator,
            &symbols,
        ).unwrap();
        let values = sorted.iter()
            .map(|reference| player.get_datum(reference).string_value(&symbols).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(values, vec!["a", "A", "b"]);
        assert_eq!(sorted[0], first_equal);
        assert_eq!(sorted[1], second_equal);

        let mut foreign_table = SymbolTable::new();
        let foreign = foreign_table.intern("foreignSort");
        let foreign_ref = player.alloc_datum(Datum::Symbol(foreign));
        assert!(sort_datums(&vec![foreign_ref], &player.allocator, &symbols).is_err());

        let mut nested_names = SymbolTable::new();
        let nested_name = nested_names.intern("foreignNestedModel");
        let nested_object = player.alloc_datum(Datum::Shockwave3dObjectRef(
            Shockwave3dObjectRef {
                cast_lib: 1,
                cast_member: 2,
                object_type: BuiltInSymbol::Model,
                name: nested_name,
            },
        ));
        let nested_list = player.alloc_datum(Datum::List(
            DatumType::List,
            VecDeque::from([nested_object]),
            false,
        ));
        assert!(sort_datums(&vec![nested_list], &player.allocator, &symbols).is_err());
    }

    #[test]
    fn sort_symbol_preflight_terminates_on_cyclic_singleton_list() {
        let mut player = test_player();
        let symbols = SymbolTable::new();
        let cycle = player.alloc_datum(Datum::List(
            DatumType::List,
            VecDeque::new(),
            false,
        ));
        *player.get_datum_mut(&cycle) = Datum::List(
            DatumType::List,
            VecDeque::from([cycle.clone()]),
            false,
        );
        assert!(sort_datums(&vec![cycle.clone()], &player.allocator, &symbols).is_ok());
        *player.get_datum_mut(&cycle) = Datum::List(DatumType::List, VecDeque::new(), false);
    }
}
