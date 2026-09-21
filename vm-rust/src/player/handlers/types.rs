use std::collections::VecDeque;
use itertools::Itertools;
use log::{debug, warn};

use crate::{
    director::lingo::datum::{Datum, DatumType, datum_bool},
    player::{
        allocator::ScriptInstanceAllocatorTrait,
        bitmap::bitmap::{get_system_default_palette, Bitmap, BuiltInPalette, PaletteRef},
        ci_string::CiStr,
        compare::sort_datums,
        datum_formatting::format_datum,
        eval::eval_lingo_expr_static,
        geometry::IntRect,
        reserve_player_mut, reserve_player_ref,
        session::ExecutionContext,
        sprite::{ColorRef, CursorRef},
        symbols::{builtin::BuiltInSymbol, symbol::Symbol, symbol_table::SymbolTable},
        xtra::manager::{get_registered_xtra_names, is_xtra_registered},
        DatumRef, DirPlayer, MathObject, ScriptError, ScriptErrorCode, XmlDocument,
    },
};

use super::datum_handlers::{
    cast_member_ref::CastMemberRefHandlers,
    date::DateObject,
    list_handlers::ListDatumHandlers,
    player_call_datum_handler,
    prop_list::{PropListDatumHandlers, PropListUtils},
    rect::RectUtils,
    sound_channel::{SoundChannelDatumHandlers, SoundStatus},
};

pub struct TypeHandlers {}
pub struct TypeUtils {}

/// Owned result of `new`. Host-backed Xtra creation remains a request for its
/// owner; script constructors carry the exact child frame and fallback value.
pub(crate) enum TypeNewPlan {
    Complete(DatumRef),
    ScriptChild {
        receiver: crate::player::script_ref::ScriptInstanceRef,
        handler_ref: crate::player::script::ScriptHandlerRef,
        args: Vec<DatumRef>,
        fallback: DatumRef,
    },
    Xtra { name: String, args: Vec<DatumRef> },
}

#[derive(Clone, Debug)]
pub(crate) struct AncestorCall {
    pub(crate) receiver: crate::player::script_ref::ScriptInstanceRef,
    /// The first ancestor to search. Handler lookup is intentionally deferred
    /// until the callback is about to run: an earlier ancestor callback may
    /// install or remove a later handler.
    pub(crate) source: crate::player::script_ref::ScriptInstanceRef,
    pub(crate) handler_name: Symbol,
    pub(crate) args: Vec<DatumRef>,
}

pub(crate) enum AncestorCallPlan {
    Complete(DatumRef),
    Children(Vec<AncestorCall>),
}

/// Resolve one deferred ancestor callback against the current owner state.
/// Returning `None` means that this ancestor chain has no matching handler and
/// the caller should continue with the next original target.
pub(crate) fn resolve_ancestor_call(
    runtime: &mut ExecutionContext<'_>,
    call: &AncestorCall,
) -> Result<Option<(
    crate::player::script_ref::ScriptInstanceRef,
    crate::player::script::ScriptHandlerRef,
    Vec<DatumRef>,
)>, ScriptError> {
    runtime.with_player_and_symbols(|player, symbols| {
        crate::player::compare::validate_direct_symbol_fields(
            &Datum::Symbol(call.handler_name.clone()),
            symbols,
        )?;
        player
            .allocator
            .get_script_instance_opt(&call.receiver)
            .ok_or_else(|| ScriptError::new_code(
                ScriptErrorCode::InvalidReference,
                "stale callAncestor receiver".to_owned(),
            ))?;
        let mut walk = call.source.clone();
        for _ in 0..100 {
            let instance = player
                .allocator
                .get_script_instance_opt(&walk)
                .ok_or_else(|| ScriptError::new_code(
                    ScriptErrorCode::InvalidReference,
                    "stale callAncestor ancestor".to_owned(),
                ))?;
            if let Some(script) = player.movie.cast_manager.get_script_by_ref(&instance.script) {
                if let Some(handler_ref) = script.get_own_handler_ref(call.handler_name.clone()) {
                    return Ok(Some((
                        call.receiver.clone(),
                        handler_ref,
                        call.args.clone(),
                    )));
                }
            }
            let Some(next) = instance.ancestor.clone() else { break };
            walk = next;
        }
        Ok(None)
    })
}

impl TypeUtils {
    pub fn get_datum_ilks(datum: &Datum) -> Result<Vec<BuiltInSymbol>, ScriptError> {
        match datum {
            Datum::List(..) => Ok(vec![BuiltInSymbol::List, BuiltInSymbol::LinearList]),
            Datum::Int(..) => Ok(vec![BuiltInSymbol::Integer]),
            Datum::Float(..) => Ok(vec![BuiltInSymbol::Float]),
            Datum::String(..) => Ok(vec![BuiltInSymbol::String]),
            Datum::Symbol(..) => Ok(vec![BuiltInSymbol::Symbol]),
            Datum::Void | Datum::Null => Ok(vec![BuiltInSymbol::Void]),
            Datum::PropList(..) => Ok(vec![BuiltInSymbol::PropList, BuiltInSymbol::List]),
            Datum::ScriptInstanceRef(..) => Ok(vec![BuiltInSymbol::Instance]),
            Datum::ScriptRef(..) => Ok(vec![BuiltInSymbol::Script]),
            Datum::CastMember(member_ref) => Ok(vec![if member_ref.is_valid() {
                BuiltInSymbol::Member
            } else {
                BuiltInSymbol::Void
            }]),
            Datum::ColorRef(..) => Ok(vec![BuiltInSymbol::Color]),
            Datum::TimeoutRef(..) => Ok(vec![BuiltInSymbol::Timeout]),
            Datum::TimeoutFactory => Ok(vec![BuiltInSymbol::Timeout]),
            Datum::TimeoutInstance { .. } => Ok(vec![BuiltInSymbol::Timeout]),
            Datum::BitmapRef(..) => Ok(vec![BuiltInSymbol::Image]),
            Datum::Rect(..) => Ok(vec![BuiltInSymbol::Rect]),
            Datum::Point(..) => Ok(vec![BuiltInSymbol::Point]),
            Datum::SpriteRef(..) => Ok(vec![BuiltInSymbol::Sprite]),
            Datum::PaletteRef(..) => Ok(vec![BuiltInSymbol::Palette]),
            Datum::Vector(..) => Ok(vec![BuiltInSymbol::Vector]),
            Datum::StringChunk(..) => Ok(vec![BuiltInSymbol::String]),
            Datum::CastLib(..) => Ok(vec![BuiltInSymbol::CastLib]),
            Datum::Stage => Ok(vec![BuiltInSymbol::Stage]),
            Datum::SoundChannel(..) => Ok(vec![BuiltInSymbol::Instance]),
            Datum::SoundRef(..) => Ok(vec![BuiltInSymbol::Sound]),
            Datum::CursorRef(..) => Ok(vec![BuiltInSymbol::Cursor]),
            Datum::Xtra(..) => Ok(vec![BuiltInSymbol::Xtra]),
            Datum::XtraInstance(..) => Ok(vec![BuiltInSymbol::Instance]),
            Datum::Matte(..) => Ok(vec![BuiltInSymbol::Image]),
            Datum::PlayerRef => Ok(vec![BuiltInSymbol::Player]),
            Datum::MovieRef => Ok(vec![BuiltInSymbol::Movie]),
            Datum::MouseRef => Ok(vec![BuiltInSymbol::Mouse]),
            Datum::XmlRef(..) => Ok(vec![BuiltInSymbol::Xml]),
            Datum::JsObjectRef(..) => Ok(vec![BuiltInSymbol::Instance]),
            Datum::DateRef(..) => Ok(vec![BuiltInSymbol::Date]),
            Datum::MathRef(..) => Ok(vec![BuiltInSymbol::Math]),
            Datum::VarRef(..) => Ok(vec![BuiltInSymbol::Void]), // VarRef should be dereferenced before checking ilk
            Datum::FlashObjectRef(..) => Ok(vec![BuiltInSymbol::Instance]),
            Datum::Shockwave3dObjectRef(r) => Ok(vec![r.object_type]),
            Datum::Transform3d(..) => Ok(vec![BuiltInSymbol::Transform]),

            _ => Err(ScriptError::new(format!(
                "Getting ilk for unknown type: {}",
                datum.type_str()
            )))?,
        }
    }

    pub fn get_datum_ilk(datum: &Datum) -> Result<BuiltInSymbol, ScriptError> {
        Ok(*Self::get_datum_ilks(datum)?.get(0).unwrap())
    }

    fn is_datum_ilk(datum: &Datum, ilk: Symbol) -> Result<bool, ScriptError> {
        Ok(Self::get_datum_ilks(datum)?
            .iter()
            .any(|x| Symbol::builtin(*x) == ilk))
    }

    pub fn get_sub_prop(
        datum_ref: &DatumRef,
        prop_key_ref: &DatumRef,
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
    ) -> Result<DatumRef, ScriptError> {
        let datum = checked_datum(player, symbols, datum_ref)?;
        let prop_key = checked_datum(player, symbols, prop_key_ref)?;
        if let Datum::Symbol(symbol) = prop_key {
            symbols
                .display(symbol)
                .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        }

        // `formatted_key` is only ever read on an error/warn path, so it is
        // formatted there rather than eagerly here — this ran a full
        // `format_datum` (and its heap allocation) on EVERY subscript read.
        let result = match datum {
            Datum::PropList(prop_list, is_sorted) => PropListUtils::get_prop(
                prop_list,
                prop_key_ref,
                &player.allocator,
                symbols,
                false,
                *is_sorted,
            )?,
            Datum::Rect(vals, flags) => {
                let index = prop_key.int_value()?; // 1..4
                let idx = (index - 1) as usize;

                if idx >= 4 {
                    return Err(ScriptError::new(format!(
                        "Rect index {} out of bounds (must be 1-4)",
                        index
                    )));
                }

                player.alloc_datum(Datum::inline_component_to_datum(vals[idx], Datum::inline_is_float(*flags, idx)))
            }
            Datum::List(_, list, _) => {
                let position = prop_key.int_value()?;
                let index = position - 1;
                if index < 0 || index >= list.len() as i32 {
                    return Err(ScriptError::new(format!("Index out of bounds: {index}")));
                }
                list[index as usize].clone()
            }
            Datum::Point(vals, flags) => {
                let index = prop_key.int_value()?;

                let idx = match index {
                    1 => 0usize,
                    2 => 1usize,
                    _ => {
                        return Err(ScriptError::new(format!(
                            "Invalid sub-prop position for point: {}",
                            index
                        )))
                    }
                };

                player.alloc_datum(Datum::inline_component_to_datum(vals[idx], Datum::inline_is_float(*flags, idx)))
            }
            // `sprite(N)[#foo]` is the bracket form of `sprite(N).foo`, and
            // Director resolves an unknown sprite property against the
            // properties of the sprite's behaviours (which `sprite_get_prop`
            // already does). Merlin's Revenge leans on it hard: every character
            // behaviour declares `pIam` naming its own state property, and the
            // shared movement code reaches it with
            //   p.spr[p.spr.pIam].w.runspeed
            //   if p.spr[#pIam] <> VOID then
            // so a symbol subscript on a sprite has to route to the same getter
            // instead of being treated as a list index.
            Datum::SpriteRef(sprite_number) => {
                let sprite_number = *sprite_number;
                let prop_name = match prop_key {
                    Datum::Symbol(name) => name.clone(),
                    Datum::String(name) => symbols.intern(name),
                    _ => {
                        return Err(ScriptError::new(format!(
                            "Cannot index sprite {} with {}",
                            sprite_number,
                            format_datum(prop_key_ref, symbols, player)?
                        )))
                    }
                };
                let result = crate::player::score::sprite_get_prop(
                    player,
                    symbols,
                    sprite_number,
                    prop_name,
                )?;
                // sprite_get_prop caches the behaviour's own DatumRef when the
                // property lives on one, so in-place mutation still reaches the
                // instance's storage rather than a clone.
                let result_ref = player
                    .last_sprite_prop_ref
                    .take()
                    .unwrap_or_else(|| player.alloc_datum(result));
                checked_datum(player, symbols, &result_ref)?;
                return Ok(result_ref);
            }
            Datum::ScriptInstanceRef(instance_ref) => {
                // A string/symbol key names an instance property.  Do this
                // before numeric indexing: Datum::int_value intentionally
                // maps a nonnumeric string to zero, which would otherwise
                // swallow a valid property lookup as an index miss.
                if let Datum::Symbol(prop_name) = prop_key {
                    symbols
                        .display(prop_name)
                        .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
                    let instance = player
                        .allocator
                        .get_script_instance_opt(instance_ref)
                        .ok_or_else(|| {
                            ScriptError::new_code(
                                crate::player::ScriptErrorCode::InvalidReference,
                                format!("invalid script instance reference {instance_ref}"),
                            )
                        })?;
                    if let Some(prop_ref) = instance.properties.get(prop_name) {
                        let prop_ref = prop_ref.clone();
                        checked_datum(player, symbols, &prop_ref)?;
                        return Ok(prop_ref);
                    }
                }

                if matches!(prop_key, Datum::String(_) | Datum::StringChunk(..)) {
                    let prop_name = prop_key.string_value(symbols)?;
                    let prop_name = symbols.intern(&prop_name);
                    let instance = player
                        .allocator
                        .get_script_instance_opt(instance_ref)
                        .ok_or_else(|| {
                            ScriptError::new_code(
                                crate::player::ScriptErrorCode::InvalidReference,
                                format!("invalid script instance reference {instance_ref}"),
                            )
                        })?;
                    if let Some(prop_ref) = instance.properties.get(&prop_name) {
                        let prop_ref = prop_ref.clone();
                        checked_datum(player, symbols, &prop_ref)?;
                        return Ok(prop_ref);
                    }
                    return Ok(DatumRef::Void);
                }

                // Numeric index
                if matches!(prop_key, Datum::Int(_) | Datum::Float(_)) {
                    let index = prop_key.int_value()?;
                    if index <= 0 {
                        return Ok(DatumRef::Void);
                    }
                    let instance = player
                        .allocator
                        .get_script_instance_opt(instance_ref)
                        .ok_or_else(|| {
                            ScriptError::new_code(
                                crate::player::ScriptErrorCode::InvalidReference,
                                format!("invalid script instance reference {instance_ref}"),
                            )
                        })?;
                    let mut property_names: Vec<(Symbol, String)> = instance
                        .properties
                        .keys()
                        .map(|key| {
                            Ok::<_, ScriptError>((
                                key.clone(),
                                symbols
                                    .display(key)
                                    .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
                                    .to_owned(),
                            ))
                        })
                        .collect::<Result<_, ScriptError>>()?;
                    property_names.sort_by(|left, right| left.1.cmp(&right.1));
                    let zero_based_index = (index - 1) as usize;

                    if zero_based_index < property_names.len() {
                        let prop_name = &property_names[zero_based_index].0;
                        if let Some(prop_ref) = instance.properties.get(prop_name) {
                            let prop_ref = prop_ref.clone();
                            checked_datum(player, symbols, &prop_ref)?;
                            return Ok(prop_ref);
                        }
                    }
                    return Ok(DatumRef::Void);
                }

                return Ok(DatumRef::Void);
            }
            Datum::Int(i) => {
                let prop_name = player.get_datum(prop_key_ref).string_value(symbols)?;
                match prop_name.as_str() {
                    "abs" => {
                        let result = i.abs();
                        player.alloc_datum(Datum::Int(result))
                    }
                    "integer" => {
                        datum_ref.clone()
                    }
                    "float" => {
                        player.alloc_datum(Datum::Float(*i as f64))
                    }
                    "char" => {
                        // Convert integer to character
                        if *i >= 0 && *i <= 255 {
                            let ch = char::from_u32(*i as u32).unwrap_or('?');
                            player.alloc_datum(Datum::String(ch.to_string()))
                        } else {
                            return Err(ScriptError::new(format!("Integer {} out of range for char", i)));
                        }
                    }
                    "string" => {
                        player.alloc_datum(Datum::String(i.to_string()))
                    }
                    _ => {
                        return Err(ScriptError::new(format!(
                            "Unknown property '{}' for integer",
                            prop_name
                        )));
                    }
                }
            }
            Datum::Float(f) => {
                let prop_name = player.get_datum(prop_key_ref).string_value(symbols)?;
                match prop_name.as_str() {
                    "abs" => {
                        let result = f.abs();
                        player.alloc_datum(Datum::Float(result))
                    }
                    "integer" => {
                        let result = f.round() as i32;
                        player.alloc_datum(Datum::Int(result))
                    }
                    "float" => {
                        datum_ref.clone()
                    }
                    "string" => {
                        player.alloc_datum(Datum::String(f.to_string()))
                    }
                    _ => {
                        return Err(ScriptError::new(format!(
                            "Unknown property '{}' for float",
                            prop_name
                        )));
                    }
                }
            }
            Datum::List(_, items, _) => {
                let index = prop_key.int_value()?;
                let idx = (index - 1) as usize;
                if idx < items.len() {
                    items[idx].clone()
                } else {
                    player.alloc_datum(Datum::Void)
                }
            }
            // Subscripting a VOID value yields VOID in Director — it never
            // raises. g349's `on particle` does `g.particles[g.cparticle]`;
            // before `g.particles` is populated (or after teardown) it is VOID,
            // so `VOID[n]` reads VOID and the subsequent `.spawn(Args)` is a
            // silent no-op on VOID rather than an error.
            Datum::Void => player.alloc_datum(Datum::Void),
            _ => {
                let formatted_key = format_datum(prop_key_ref, symbols, player)?;
                web_sys::console::log_1(
                    &format!(
                        "  ❌ Cannot get sub-prop '{}' from type {}",
                        formatted_key,
                        datum.type_str()
                    )
                    .into(),
                );
                return Err(ScriptError::new(format!(
                    "Cannot get sub-prop `{}` from prop of type {}",
                    formatted_key,
                    datum.type_str()
                )));
            }
        };
        checked_datum(player, symbols, &result)?;
        Ok(result)
    }

    pub fn set_sub_prop(
        datum_ref: &DatumRef,
        prop_key_ref: &DatumRef,
        value_ref: &DatumRef,
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
    ) -> Result<(), ScriptError> {
        let datum_type = checked_datum(player, symbols, datum_ref)?.type_enum();
        match datum_type {
            DatumType::PropList => {
                PropListUtils::set_prop(datum_ref, prop_key_ref, value_ref, player, symbols, false)
            }
            DatumType::List => {
                let position = checked_datum(player, symbols, prop_key_ref)?.int_value()?;
                let index = position - 1;
                checked_datum(player, symbols, value_ref)?;
                let (_, list, _) = player.get_datum_mut(datum_ref).to_list_mut().unwrap();
                if index < 0 {
                    return Err(ScriptError::new(format!("Index out of bounds: {index}")));
                } else if index < list.len() as i32 {
                    list[index as usize] = value_ref.clone();
                } else {
                    // FIXME this is not the same as Director, which would fill in the list with zeros
                    list.resize((index as usize + 1).max(list.len()), DatumRef::Void);
                    list[index as usize] = value_ref.clone();
                }
                Ok(())
            }
            DatumType::Rect => {
                let position = checked_datum(player, symbols, prop_key_ref)?.int_value()?;
                let index = (position - 1) as usize;
                if index >= 4 {
                    return Err(ScriptError::new(format!("Rect index out of bounds: {position}")));
                }
                let new_val = checked_datum(player, symbols, value_ref)?.clone();
                let (component_val, is_float) = Datum::datum_to_inline_component(&new_val)?;
                let (vals, flags) = player.get_datum_mut(datum_ref).to_rect_inline_mut()?;
                vals[index] = component_val;
                Datum::inline_set_float(flags, index, is_float);
                Ok(())
            }
            _ => {
                let formatted_key = format_datum(prop_key_ref, symbols, player)?;
                warn!(
                    "⚠️ Cannot set sub-prop `{}` on prop of type {} (ignored)",
                    formatted_key,
                    datum_type.type_str()
                );
                Ok(())
            }
        }
    }
}

/// Resolve a datum through the context-owned allocator and reject handles or
/// direct symbols owned by another session before a synchronous type handler
/// consumes the value.  Ordinary type/coercion failures remain the caller's
/// existing fallback behavior.
fn checked_datum<'a>(
    player: &'a DirPlayer,
    symbols: &SymbolTable,
    datum_ref: &DatumRef,
) -> Result<&'a Datum, ScriptError> {
    let datum = match datum_ref {
        DatumRef::Void => &Datum::Void,
        _ => player
            .allocator
            .try_get_datum(datum_ref)
            .ok_or_else(|| {
                ScriptError::new_code(
                    crate::player::ScriptErrorCode::InvalidReference,
                    format!("invalid datum reference {datum_ref}"),
                )
            })?,
    };
    crate::player::compare::validate_direct_symbol_fields(datum, symbols)?;
    Ok(datum)
}

/// Extract the source text accepted by Director's `value()` builtin.  The
/// caller owns the player and symbol table for this short check; the returned
/// text is copied before any evaluator request can suspend.
pub(crate) fn value_source_text(
    player: &DirPlayer,
    symbols: &SymbolTable,
    datum_ref: &DatumRef,
) -> Result<Option<String>, ScriptError> {
    crate::player::driver::validate_owned_datum_graph(player, symbols, datum_ref)?;
    let datum = checked_datum(player, symbols, datum_ref)?;
    match datum {
        Datum::String(value) => Ok(Some(value.clone())),
        Datum::StringChunk(..) => Ok(Some(datum.string_value(symbols)?)),
        _ => Ok(None),
    }
}

fn get_script_instance_prop_explicit(
    player: &mut DirPlayer,
    symbols: &mut SymbolTable,
    datum_ref: &DatumRef,
    args: &Vec<DatumRef>,
) -> Result<DatumRef, ScriptError> {
    let prop_key = checked_datum(player, symbols, args.first().ok_or_else(|| {
        ScriptError::new("get_a_prop requires a property name".to_owned())
    })?)?;
    let prop_name = match prop_key {
        Datum::Symbol(name) => name.clone(),
        _ => {
            let prop_text = prop_key.string_value(symbols)?;
            symbols.intern(&prop_text)
        }
    };
    let instance_ref = checked_datum(player, symbols, datum_ref)?
        .to_script_instance_ref()?
        .clone();
    let prop_ref = crate::player::script::script_get_prop_opt(
        player,
        symbols,
        &instance_ref,
        prop_name,
    )?
    .unwrap_or(DatumRef::Void);
    checked_datum(player, symbols, &prop_ref)?;
    Ok(prop_ref)
}

impl TypeHandlers {
    /// Owner-bound constructor for the native Multiuser/Curl subset. The
    /// legacy async `new` entrypoint remains for script/cast construction and
    /// external plugin loading; session-owned dispatch can use this method to
    /// construct an Xtra without consulting process-global managers.
    pub(crate) fn new_explicit(
        runtime: &mut ExecutionContext<'_>,
        args: &[DatumRef],
    ) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let subject_ref = args
                .first()
                .ok_or_else(|| ScriptError::new("new requires an object type".to_owned()))?;
            let subject = checked_datum(player, symbols, subject_ref)?.clone();
            let Datum::Xtra(xtra_name) = subject else {
                return Err(ScriptError::new(
                    "owner-bound Xtra construction requires an Xtra factory".to_owned(),
                ));
            };
            let id = crate::player::xtra::manager::create_xtra_instance_explicit(
                player,
                symbols,
                &xtra_name,
                &args[1..],
            )?;
            Ok(player.alloc_datum(Datum::XtraInstance(xtra_name, id)))
        })
    }

    pub fn objectp(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let obj = checked_datum(player, symbols, &args[0])?;
            let is_object = match obj {
                Datum::Void => false,
                Datum::Float(_) => false,
                Datum::Int(_) => false,
                Datum::Symbol(_) => false,
                Datum::String(_) => false,
                _ => true,
            };
            Ok(player.alloc_datum(datum_bool(is_object)))
        })
    }

    pub fn voidp(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let obj = checked_datum(player, symbols, &args[0])?;
            let is_void = match obj {
                Datum::Void => true,
                _ => false,
            };
            Ok(player.alloc_datum(datum_bool(is_void)))
        })
    }

    pub fn listp(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let obj = checked_datum(player, symbols, &args[0])?;
            let is_list = match obj {
                Datum::List(..) => true,
                Datum::PropList(..) => true,
                _ => false,
            };
            Ok(player.alloc_datum(datum_bool(is_list)))
        })
    }

    pub fn symbolp(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let obj = checked_datum(player, symbols, &args[0])?;
            let is_symbol = match obj {
                Datum::Symbol(_) => true,
                _ => false,
            };
            Ok(player.alloc_datum(datum_bool(is_symbol)))
        })
    }

    pub fn stringp(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let obj = checked_datum(player, symbols, &args[0])?;
            let is_string = match obj {
                Datum::String(_) => true,
                Datum::StringChunk(..) => true,
                _ => false,
            };
            Ok(player.alloc_datum(datum_bool(is_string)))
        })
    }

    pub fn integerp(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let obj = checked_datum(player, symbols, &args[0])?;
            let is_integer = match obj {
                Datum::Int(_) => true,
                _ => false,
            };
            Ok(player.alloc_datum(datum_bool(is_integer)))
        })
    }

    pub fn floatp(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let obj = checked_datum(player, symbols, &args[0])?;
            let is_float = match obj {
                Datum::Float(_) => true,
                _ => false,
            };
            Ok(player.alloc_datum(datum_bool(is_float)))
        })
    }

    pub fn value(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let eval_expr = runtime.with_player_and_symbols(|player, symbols| -> Result<_, ScriptError> {
            let datum = checked_datum(player, symbols, &args[0])?;
            let value = match datum {
                Datum::String(s) => Some(s.clone()),
                // StringChunk: produce the chunk's resolved text and
                // parse that as Lingo, same as a plain string. Habbo
                // / Sulake movies wrap field lookups in `value(...)`:
                // `value(convertToPropList(field(...))["object.manager.class"])[1]`
                // — without this branch, value() returned the chunk
                // unchanged and `[1]` then errored with
                // "No handler getAt for string chunk datum".
                Datum::StringChunk(..) => {
                    datum.string_value(symbols).ok()
                },
                _ => None,
            };
            Ok(value)
        })?;
        match eval_expr {
            Some(s) => {
                // Match the string-property `.value` cleanup — Coke Studios'
                // ElementManager splits elements XML at every "]" and feeds
                // partial fragments to value() until one parses (the Lingo
                // relies on Void-on-failure to drive its retry loop). Without
                // normalising comments and unbalanced brackets here, every
                // partial attempt dumps a pest parse error to the console.
                use crate::player::handlers::datum_handlers::string::{
                    normalise_lingo_expr_for_value,
                };
                let cleaned = normalise_lingo_expr_for_value(&s);
                // Director's `value()` evaluates the FIRST complete expression
                // and ignores any trailing tokens. When the input is a list /
                // property list, trim to the matching close bracket so trailing
                // garbage doesn't fail the whole parse. Summer Resort's room
                // members are Paige #text stored in over-allocated text blocks:
                // the buffer holds the real room list followed by stale bytes
                // left from a prior edit (Paige tracks the logical length but
                // XMED serializes the whole buffer). Parsing the whole thing
                // returned VOID, which blanked the room (and broke map
                // navigation, since the `= #empty` boundary check then let the
                // player scroll into an unparsed/black room). Trimming to the
                // first balanced list matches Director and recovers the room.
                let cleaned = truncate_to_first_balanced_list(&cleaned);
                // `value("")` is 0, not VOID.
                //
                // The 11.5 Scripting Dictionary entry for value() says an
                // unparseable expression yields "the value of the initial
                // portion of the expression up to the first syntax error" —
                // for an empty string that initial portion is nothing, which
                // evaluates to 0. (The doc spells out `value("penny")` → VOID
                // for an unresolvable IDENTIFIER, but is silent on the empty
                // case, so the 0 here is inferred, not quoted.)
                //
                // Habbo v31's furnidata pins it down: every WALL item ("i")
                // stores its direction / xdim / ydim as empty fields —
                //   ["i","1391","window_skyscraper","7339","","","","",...]
                // — and the Persistent Furni Data Container does
                // `tdata[#defaultDir] = value(tItem[5])`. The catalogue's
                // renderLargePreviewImage then only tests `voidp(direction)`
                // before overwriting it with a hardcoded "2,2,2", so a VOID
                // here aborted the preview for EVERY wall item (windows,
                // posters, wallpaper) with "Direction property missing".
                // Returning 0 also keeps `xdim & "," & ydim` a parseable
                // "0,0" for the dimensions list that follows.
                //
                // Gate on the ORIGINAL string being empty, NOT the normalised
                // one. Normalisation deliberately strips unbalanced brackets,
                // so `value("]")` also cleans to "" — and Coke Studios'
                // ElementManager depends on THAT staying Void: it splits the
                // window XML on "]" and feeds each fragment to value() until
                // one parses, using `if not voidp(aElement)` to decide whether
                // to keep it. The split leaves an empty tail, so the last
                // fragment is exactly "]"; returning 0 for it appended a bare 0
                // to the element list and `myelement.id` then failed with
                // "Cannot get int property id". `is_expected_value_retry_fragment`
                // below lists "]" as an expected fragment for the same reason.
                if s.trim().is_empty() {
                    return Ok(runtime.player.alloc_datum(Datum::Int(0)));
                }
                // TEMP diagnostic: log EVERY value() call that looks like a
                // Lingo prop-list/list so we can confirm whether the Coke
                // Studios ElementManager retry (concatenating the two halves
                // of a split-by-"]" element) produces a valid parse.
                let is_list_or_proplist_input = {
                    let t = s.trim_start();
                    t.starts_with("[#") || t.starts_with("[")
                };
                match runtime.with_player_and_symbols(|player, symbols| {
                    eval_lingo_expr_static(cleaned.clone(), player, symbols)
                }) {
                    Ok(datum_ref) => {
                        if is_list_or_proplist_input {
                            debug!(
                                "[value() OK] input={:?}",
                                s.chars().take(140).collect::<String>()
                            );
                        }
                        Ok(datum_ref)
                    }
                    Err(err) => {
                        if !is_expected_value_retry_fragment(&s, &cleaned) {
                            warn!(
                                "[value()] parse error → Void — input={:?} cleaned={:?} err={}",
                                s, cleaned, &err.message
                            );
                        }
                        Ok(DatumRef::Void)
                    }
                }
            }
            _ => Ok(args[0].clone()),
        }
    }

    pub fn void(_: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        Ok(DatumRef::Void)
    }

    pub fn ilk(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let result_datum = {
            let player = &*runtime.player;
            let obj = checked_datum(player, &*runtime.symbols, &args[0])?;
            if let Some(query_ref) = args.get(1) {
                let query_symbol = if let Datum::Symbol(symbol) =
                    checked_datum(player, &*runtime.symbols, query_ref)?
                {
                    symbol.clone()
                } else {
                    let text = checked_datum(player, &*runtime.symbols, query_ref)?
                        .string_value(&*runtime.symbols)?;
                    // `ilk()` compares a string query after interning it in
                    // the session table, preserving symbol identity for
                    // subsequent builtin checks.
                    runtime.symbols.intern(&text)
                };
                datum_bool(TypeUtils::is_datum_ilk(obj, query_symbol)?)
            } else {
                Datum::Symbol(Symbol::builtin(TypeUtils::get_datum_ilk(obj)?))
            }
        };
        Ok(runtime.player.alloc_datum(result_datum))
    }

    pub(crate) fn integer_impl(input: &str) -> Option<i32> {
        if input.is_empty() {
            return None;
        }

        // Remove leading and trailing whitespace
        let trimmed_input = input.trim();

        if trimmed_input.is_empty() {
            return Some(0);
        }

        if trimmed_input == "-" {
            return Some(0);
        }

        let mut result = String::new();
        let mut found_valid_digit = false;

        for char in trimmed_input.chars() {
            match char {
                // numeric_chars
                '0' | '1' | '2' | '3' | '4' | '5' | '6' | '7' | '8' | '9' => {
                    result.push(char);
                    found_valid_digit = true;
                }
                // special_symbols
                '.' => return None,
                '-' => {
                    if result.is_empty() {
                        result.push(char);
                    } else {
                        return None;
                    }
                }
                // unknown
                _ => {
                    if !found_valid_digit {
                        return None;
                    }
                }
            };
        }

        if !found_valid_digit {
            return None;
        }

        // Convert result to integer
        if let Ok(final_result) = result.parse::<i32>() {
            return Some(final_result);
        }

        // Director Lingo treats an all-digit string (optionally signed) as an
        // integer even when the value exceeds i32 range — Habbo's parseFigure
        // calls `integerp(integer(figure))` on a 25-digit figure string just to
        // confirm it's numeric, and relies on `integerp` returning true. Match
        // that by saturating the parse so the result type stays Int rather
        // than collapsing to Void.
        if let Ok(big) = result.parse::<i64>() {
            return Some(big.clamp(i32::MIN as i64, i32::MAX as i64) as i32);
        }
        // Even larger than i64: saturate based on sign.
        let is_negative = result.starts_with('-');
        Some(if is_negative { i32::MIN } else { i32::MAX })
    }

    pub(crate) fn float_impl(input: &str) -> Option<f64> {
        if input.is_empty() {
            return None;
        }

        let trimmed_input = input.trim();

        if trimmed_input.is_empty() {
            return Some(0.0);
        }

        if trimmed_input == "-" {
            return Some(0.0);
        }

        let mut result = String::new();
        let mut found_valid_digit = false;
        let mut found_decimal = false;

        for char in trimmed_input.chars() {
            match char {
                '0' | '1' | '2' | '3' | '4' | '5' | '6' | '7' | '8' | '9' => {
                    result.push(char);
                    found_valid_digit = true;
                }
                '.' => {
                    if found_decimal {
                        return None;
                    }
                    result.push(char);
                    found_decimal = true;
                }
                '-' => {
                    if result.is_empty() {
                        result.push(char);
                    } else {
                        return None;
                    }
                }
                // Unlike integer(), Director's float() is all-or-nothing: any
                // character that isn't part of the number makes the conversion
                // fail and float() hands back its argument untouched — in
                // Director `put float("5;2;13;1;0;0;")` prints the string back.
                // Junkbot's config manager relies on that, feeding part strings
                // through float() and only treating the result as numeric when
                // the conversion succeeded. Skipping the separators the way
                // integer_impl does would turn that line into 52131000.
                _ => return None,
            };
        }

        if !found_valid_digit {
            return None;
        }

        result.parse::<f64>().ok()
    }

    pub fn integer(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let value = checked_datum(player, symbols, &args[0])?;
            let result = match value {
                Datum::Int(i) => Datum::Int(*i),
                Datum::Float(f) => Datum::Int(f.round() as i32),
                Datum::SpriteRef(sprite_num) => Datum::Int(*sprite_num as i32),
                Datum::String(s) => {
                    let result = Self::integer_impl(&s);
                    if let Some(int_value) = result {
                        Datum::Int(int_value)
                    } else {
                        return Ok(DatumRef::Void);
                    }
                }
                // StringChunk is what `member.line[N].item[M]` and friends
                // now resolve to (since chunk-typed getProp returns refs so
                // chained property reads like `.font` work). For
                // `integer(...)` and other numeric coercions, fall through
                // to the StringChunk's resolved string value and parse.
                // Fugue No.4 Cues#startMovie: `lm = integer(member("ClikPts")
                // .line[2].item[1])`.
                Datum::StringChunk(_, _, resolved) => {
                    let result = Self::integer_impl(resolved);
                    if let Some(int_value) = result {
                        Datum::Int(int_value)
                    } else {
                        return Ok(DatumRef::Void);
                    }
                }
                Datum::DateRef(date_id) => {
                    // Director's `integer(the systemDate)` returns the date
                    // packed as YYYYMMDD (e.g. 20240328 for 28 Mar 2024).
                    // Used by Director scripts as a numeric handshake/seed
                    // — e.g. ClubMarian's login flow does
                    // `key3 = bitXor(integer(the systemDate), key1)`.
                    let date_obj = player
                        .date_objects
                        .get(date_id)
                        .ok_or_else(|| ScriptError::new(format!(
                            "Date object {} not found", date_id
                        )))?;
                    let js_date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(
                        date_obj.timestamp_ms as f64,
                    ));
                    let yyyymmdd = js_date.get_full_year() as i32 * 10000
                        + (js_date.get_month() as i32 + 1) * 100  // js months are 0-based
                        + js_date.get_date() as i32;
                    Datum::Int(yyyymmdd)
                }
                Datum::Void => Datum::Void,
                _ => {
                    return Err(ScriptError::new(format!(
                        "Cannot convert datum of type {} to integer",
                        value.type_str()
                    )))
                }
            };
            Ok(player.alloc_datum(result))
        })
    }

    pub fn float(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let value = checked_datum(player, symbols, &args[0])?;
            let result = match value {
                Datum::Float(f) => Datum::Float(*f),
                Datum::Int(i) => Datum::Float(*i as f64),
                Datum::SpriteRef(sprite_num) => Datum::Float(*sprite_num as f64),
                Datum::String(s) => {
                    if let Some(float_value) = Self::float_impl(s) {
                        Datum::Float(float_value)
                    } else {
                        value.to_owned()
                    }
                }
                Datum::StringChunk(_, _, s) => {
                    if let Some(float_value) = Self::float_impl(s) {
                        Datum::Float(float_value)
                    } else {
                        value.to_owned()
                    }
                }
                Datum::Void => Datum::Void,
                // Director returns a point unchanged rather than converting or
                // erroring — verified in Director:
                //   p = point(-342, 159)
                //   put float(p * p)  -- point(116964, 25281)
                // (components stay integers). NabiscoWorld Mini Mini-Golf's
                // Hole 18 `obstacle` handler relies on this: it computes
                // `sqhv = float(dhv * dhv)` on a point and then reads
                // `sqhv.locH` / `sqhv.locV`, so float() must yield a point.
                // Same leniency as the String arm above, which returns the
                // input unchanged when it doesn't parse as a number.
                Datum::Point(..) => value.to_owned(),
                _ => {
                    return Err(ScriptError::new(format!(
                        "Cannot convert datum of type {} to float",
                        value.type_str()
                    )))
                }
            };
            Ok(player.alloc_datum(result))
        })
    }

    pub fn symbol(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let symbol_name = checked_datum(runtime.player, &*runtime.symbols, &args[0])?;
        let result = if let Datum::Symbol(symbol) = symbol_name {
            Datum::Symbol(symbol.clone())
        } else if let Datum::Void = symbol_name {
            Datum::Symbol(Symbol::builtin(BuiltInSymbol::Void))
        } else if symbol_name.is_string() {
            let str_value = symbol_name.string_value(&*runtime.symbols)?;
            let trimmed = str_value.trim();
            let symbol = if trimmed.is_empty() {
                Symbol::builtin(BuiltInSymbol::EmptyString)
            } else if trimmed.starts_with("#") {
                runtime.symbols.intern(&trimmed[1..])
            } else {
                runtime.symbols.intern(trimmed)
            };
            Datum::Symbol(symbol)
        } else {
            return Err(ScriptError::new(format!(
                "Cannot convert datum of type {} to symbol",
                symbol_name.type_str()
            )));
        };
        Ok(runtime.player.alloc_datum(result))
    }

    pub fn point(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            if args.len() != 2 {
                return Err(ScriptError::new("point() requires exactly 2 arguments".to_string()));
            }

            let x = checked_datum(player, symbols, &args[0])?.clone();
            let y = checked_datum(player, symbols, &args[1])?.clone();
            let point = Datum::build_point(&x, &y)?;
            Ok(player.alloc_datum(point))
        })
    }

    pub fn rect(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            if args.len() != 2 && args.len() != 4 {
                return Err(ScriptError::new("rect() requires 2 or 4 arguments".to_string()));
            }

            // Case 1: rect(left, top, right, bottom)
            if args.len() == 4 && checked_datum(player, symbols, &args[0])?.is_number() {
                let l = checked_datum(player, symbols, &args[0])?.clone();
                let t = checked_datum(player, symbols, &args[1])?.clone();
                let r = checked_datum(player, symbols, &args[2])?.clone();
                let b = checked_datum(player, symbols, &args[3])?.clone();
                let rect = Datum::build_rect(&l, &t, &r, &b)?;
                return Ok(player.alloc_datum(rect));
            }

            // Case 2: rect(Point, Point)
            if args.len() == 2 {
                let (p1, f1) = checked_datum(player, symbols, &args[0])?.to_point_inline()?;
                let (p2, f2) = checked_datum(player, symbols, &args[1])?.to_point_inline()?;

                let flags = (if Datum::inline_is_float(f1, 0) { 1u8 } else { 0 })
                    | (if Datum::inline_is_float(f1, 1) { 2u8 } else { 0 })
                    | (if Datum::inline_is_float(f2, 0) { 4u8 } else { 0 })
                    | (if Datum::inline_is_float(f2, 1) { 8u8 } else { 0 });

                return Ok(player.alloc_datum(Datum::Rect([p1[0], p1[1], p2[0], p2[1]], flags)));
            }

            Err(ScriptError::new("Invalid rect() arguments".to_string()))
        })
    }

    pub fn cursor(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            if args.len() == 1 {
                let arg = checked_datum(player, symbols, &args[0])?;
                if arg.is_int() {
                    let cursor_val = arg.int_value()?;
                    player.cursor = CursorRef::System(cursor_val);
                    if cursor_val == 200 || cursor_val == -1 {
                        player.cursor_is_hidden = true;
                    } else {
                        player.cursor_is_hidden = false;
                        player.wants_pointer_lock = false;
                    }
                    Ok(DatumRef::Void)
                } else if arg.is_list() {
                    let list = arg.to_list()?;
                    let mut members = vec![];
                    for item in list {
                        let datum = checked_datum(player, symbols, item)?;
                        let slot = match datum {
                            Datum::CastMember(member_ref) => {
                                CastMemberRefHandlers::get_cast_slot_number(
                                    member_ref.cast_lib as u32,
                                    member_ref.cast_member as u32,
                                ) as i32
                            }
                            _ => datum.int_value()?,
                        };
                        members.push(slot);
                    }
                    player.cursor = CursorRef::Member(members);
                    player.cursor_is_hidden = false;
                    player.wants_pointer_lock = false;
                    Ok(DatumRef::Void)
                } else if let Datum::CastMember(member_ref) = arg {
                    // Director accepts a single cast member directly:
                    //   cursor(member("HandCursor"))
                    // Treated as a one-element list — equivalent to
                    //   cursor([member("HandCursor")])
                    let slot = CastMemberRefHandlers::get_cast_slot_number(
                        member_ref.cast_lib as u32,
                        member_ref.cast_member as u32,
                    ) as i32;
                    player.cursor = CursorRef::Member(vec![slot]);
                    player.cursor_is_hidden = false;
                    player.wants_pointer_lock = false;
                    Ok(DatumRef::Void)
                } else {
                    Err(ScriptError::new("Invalid argument for cursor".to_string()))
                }
            } else if args.len() == 2 {
                Err(ScriptError::new("Cursor call not implemented".to_string()))
            } else {
                Err(ScriptError::new(
                    "Invalid number of arguments for cursor".to_string(),
                ))
            }
        })
    }

    pub fn prepare_new(
        runtime: &mut ExecutionContext<'_>,
        args: &[DatumRef],
    ) -> Result<TypeNewPlan, ScriptError> {
        if args.is_empty() {
            return Err(ScriptError::new("new requires at least one argument".to_owned()));
        }
        runtime.with_player_and_symbols(|player, symbols| {
            let subject = checked_datum(player, symbols, &args[0])?.clone();
            match subject {
                Datum::Symbol(_) => {
                    let name = subject.string_value(symbols)?;
                    let (cast_num, slot) = if let Some(location) = args.get(1) {
                        match checked_datum(player, symbols, location)? {
                            Datum::CastLib(cast_num) => (*cast_num, None),
                            Datum::CastMember(member_ref) => (
                                if member_ref.cast_lib > 0 { member_ref.cast_lib as u32 } else { 1 },
                                (member_ref.cast_member > 0).then_some(member_ref.cast_member as u32),
                            ),
                            other => return Err(ScriptError::new(format!(
                                "Unsupported new() location type: {}", other.type_str(),
                            ))),
                        }
                    } else {
                        (1, None)
                    };
                    let cast = player.movie.cast_manager.get_cast_mut(cast_num);
                    let member_slot = slot.unwrap_or_else(|| cast.first_free_member_id());
                    if member_slot == 0 {
                        return Err(ScriptError::new(format!(
                            "new({}): cast library {} has no free member slots", name, cast_num,
                        )));
                    }
                    let member_ref = cast.create_member_at(
                        member_slot,
                        &name,
                        &mut player.bitmap_manager,
                        symbols,
                    )?;
                    player.movie.cast_manager.invalidate_member_name_cache();
                    Ok(TypeNewPlan::Complete(player.alloc_datum(Datum::CastMember(member_ref))))
                }
                Datum::ScriptRef(script_ref) => {
                    use crate::player::handlers::datum_handlers::script::{
                        ScriptConstructorPlan, ScriptDatumHandlers,
                    };
                    let plan = ScriptDatumHandlers::prepare_constructor(
                        player,
                        symbols,
                        &args[0],
                        &args[1..],
                        BuiltInSymbol::New,
                    )?;
                    Ok(match plan {
                        ScriptConstructorPlan::Complete(result) => TypeNewPlan::Complete(result),
                        ScriptConstructorPlan::Child { receiver, handler_ref, args, fallback } =>
                            TypeNewPlan::ScriptChild { receiver, handler_ref, args, fallback },
                    })
                }
                Datum::Xtra(name) => Ok(TypeNewPlan::Xtra {
                    name,
                    args: args.to_vec(),
                }),
                other => Err(ScriptError::new(format!(
                    "Unsupported new call with subject type: {}", other.type_enum().type_str(),
                ))),
            }
        })
    }

    pub fn finish_script_new(
        runtime: &mut ExecutionContext<'_>,
        fallback: DatumRef,
        result: DatumRef,
    ) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            crate::player::handlers::datum_handlers::script::ScriptDatumHandlers::finish_constructor(
                player, symbols, fallback, result,
            )
        })
    }

    pub fn timeout(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            if args.is_empty() {
                // Called without arguments: return the timeout factory
                Ok(player.alloc_datum(Datum::TimeoutFactory))
            } else {
                // Called with a name argument: return a timeout reference
                let name = checked_datum(player, symbols, &args[0])?.string_value(symbols)?;
                Ok(player.alloc_datum(Datum::TimeoutRef(name)))
            }
        })
    }

    pub fn rgb(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            if args.len() == 3 {
                let r = checked_datum(player, symbols, &args[0])?.int_value()? as u8;
                let g = checked_datum(player, symbols, &args[1])?.int_value()? as u8;
                let b = checked_datum(player, symbols, &args[2])?.int_value()? as u8;
                Ok(player.alloc_datum(Datum::ColorRef(ColorRef::Rgb(r, g, b))))
            } else {
                let first_arg = checked_datum(player, symbols, &args[0])?;
                if first_arg.is_string() {
                    let hex_str = first_arg.string_value(symbols)?.replace("#", "");
                    let r_str = if hex_str.len() >= 2 { &hex_str[0..2] } else { "00" };
                    let g_str = if hex_str.len() >= 4 { &hex_str[2..4] } else { "00" };
                    let b_str = if hex_str.len() >= 6 { &hex_str[4..6] } else { "00" };
                    
                    let r = u8::from_str_radix(r_str, 16).unwrap_or(0);
                    let g = u8::from_str_radix(g_str, 16).unwrap_or(0);
                    let b = u8::from_str_radix(b_str, 16).unwrap_or(0);
                    Ok(player.alloc_datum(Datum::ColorRef(ColorRef::Rgb(r, g, b))))
                } else {
                    Err(ScriptError::new(
                        "Invalid number of arguments for rgb".to_string(),
                    ))
                }
            }
        })
    }

    /// Director chapter 15 `filter(filterSymbol [, propList])` global.
    /// Source: <c>director_reference.md:24348</c>. Constructs a filter object
    /// that can be passed to `image.applyFilter()`. The filter is represented
    /// as a PropList with a `#filterType` entry plus the user-supplied
    /// properties merged in. Director scripts treat the filter as opaque,
    /// but this layout makes the filter easy to inspect from Lingo and lets
    /// `applyFilter` dispatch by `#filterType`.
    ///
    /// Supported filter symbols (per chapter 15):
    ///   #blurfilter, #glowfilter, #bevelfilter, #dropshadowfilter,
    ///   #adjustcolorfilter, #gradientglowfilter, #gradientbevelfilter,
    ///   #convolutionmatrixfilter, #displacementmapfilter
    /// Currently only `#adjustcolorfilter` is honored by `applyFilter`; the
    /// others are accepted (so scripts don't error out building them) but the
    /// applyFilter step is a no-op for them and logs a warning.
    pub fn filter(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        if args.is_empty() {
            return Err(ScriptError::new("filter() requires a filter symbol".to_string()));
        }
        let kind_symbol = match checked_datum(runtime.player, &*runtime.symbols, &args[0])? {
            Datum::Symbol(symbol) => symbol.clone(),
            Datum::String(name) => runtime.symbols.intern(name),
            _ => return Err(ScriptError::new(
                "filter() first argument must be a symbol".to_string(),
            )),
        };
        runtime.with_player_and_symbols(|player, symbols| {
            // Second arg (optional): property list with filter-specific params.
            let mut props: VecDeque<(DatumRef, DatumRef)> = VecDeque::new();

            // Insert #filterType first.
            let key_type = player.alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::FilterType)));
            let val_type = player.alloc_datum(Datum::Symbol(kind_symbol.clone()));
            props.push_back((key_type, val_type));

            if args.len() > 1 {
                // Clone the user PropList entries verbatim. We don't validate
                // property names against the filter kind — Director's `filter()`
                // is similarly permissive and just stashes whatever it gets.
                let user_props_owned = match checked_datum(player, symbols, &args[1])? {
                    Datum::PropList(items, _) => Some(items.clone()),
                    _ => None,
                };
                if let Some(items) = user_props_owned {
                    for (k, v) in items.into_iter() {
                        checked_datum(player, symbols, &k)?;
                        checked_datum(player, symbols, &v)?;
                        props.push_back((k, v));
                    }
                } else {
                    // Non-PropList second arg — accept but ignore (Director silently
                    // discards malformed second-arg).
                    warn!("filter(): second argument is not a property list — ignored");
                }
            }

            Ok(player.alloc_datum(Datum::PropList(props, false)))
        })
    }

    /// Director chapter 15 `newMatrix(rows, columns [, initialList])` — math /
    /// terrain matrix constructor (`director_reference.md:1060`,
    /// `director_reference.md:31053`). Returns a `rows × columns` matrix
    /// initialized to zero, or filled from `initialList` (a flat
    /// row-major list of `rows*columns` numbers).
    ///
    /// Layout: a Lingo `Datum::List` of `rows` rows, each itself a
    /// `Datum::List` of `columns` numeric entries. This makes the matrix
    /// directly compatible with our `createTerrainDesc(matrix, ...)` handler
    /// which expects a list-of-lists for the elevation matrix, and lets the
    /// `setVal`/`getVal` matrix methods reuse the existing list-mutation
    /// machinery.
    pub fn new_matrix(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            if args.len() < 2 {
                return Err(ScriptError::new(
                    "newMatrix requires at least 2 arguments (rows, columns)".to_string(),
                ));
            }
            let rows = checked_datum(player, symbols, &args[0])?.int_value()?.max(0) as usize;
            let cols = checked_datum(player, symbols, &args[1])?.int_value()?.max(0) as usize;

            // Optional 3rd arg: flat row-major list of values.
            let init_values: Option<Vec<f64>> = if args.len() > 2 {
                match checked_datum(player, symbols, &args[2])? {
                    Datum::List(_, items, _) => {
                        let mut vals = Vec::with_capacity(items.len());
                        for it in items.iter() {
                            vals.push(checked_datum(player, symbols, it)?.float_value().unwrap_or(0.0));
                        }
                        Some(vals)
                    }
                    _ => None,
                }
            } else { None };

            // Build rows × cols list of lists.
            let mut row_refs: VecDeque<DatumRef> = VecDeque::with_capacity(rows);
            for r in 0..rows {
                let mut col_refs: VecDeque<DatumRef> = VecDeque::with_capacity(cols);
                for c in 0..cols {
                    let val = match &init_values {
                        Some(v) => {
                            let idx = r * cols + c;
                            if idx < v.len() { v[idx] } else { 0.0 }
                        }
                        None => 0.0,
                    };
                    // Store as Float when fractional, else Int — matches
                    // Director's behaviour on numeric matrix entries.
                    let cell = if val.fract() == 0.0 && val.abs() < (i32::MAX as f64) {
                        Datum::Int(val as i32)
                    } else {
                        Datum::Float(val)
                    };
                    col_refs.push_back(player.alloc_datum(cell));
                }
                row_refs.push_back(player.alloc_datum(Datum::List(DatumType::List, col_refs, false)));
            }
            Ok(player.alloc_datum(Datum::List(DatumType::List, row_refs, false)))
        })
    }

    /// Director chapter 15 `ConstraintDesc(name, A, B, ptA, ptB, stiffness, damping)`
    /// (`director_reference.md:84716`). Builds an opaque descriptor consumed by
    /// `world.createSpring(desc, ...)` / `createLinearJoint` / `createAngularJoint` /
    /// `createD6Joint`.
    ///
    /// Director's docs describe the result as an opaque object — Lingo
    /// scripts only ever pass it back to the create* call and never read
    /// individual fields. We represent it as a `Datum::List` of the 7
    /// arguments, which the constraint-create handlers in `physx.rs`
    /// already decode via `decode_desc` (List form). The 3rd arg may be
    /// `void` when constraining a body to a fixed point in world space
    /// (chapter 15:84768).
    pub fn constraint_desc(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            if args.len() < 7 {
                return Err(ScriptError::new(
                    "ConstraintDesc requires (name, A, B, ptA, ptB, stiffness, damping)".to_string(),
                ));
            }
            for arg in args.iter().take(7) {
                checked_datum(player, symbols, arg)?;
            }
            // Just bundle the args into a List — the create* handlers decode
            // them. We could validate types up front, but Director itself is
            // permissive and stores whatever the script passes in (the
            // create* call surfaces any error later).
            let items: VecDeque<DatumRef> = args.iter().take(7).cloned().collect();
            Ok(player.alloc_datum(Datum::List(DatumType::List, items, false)))
        })
    }

    pub fn palette_index(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let color = checked_datum(player, symbols, &args[0])?.int_value()?;
            Ok(player.alloc_datum(Datum::ColorRef(ColorRef::PaletteIndex(color as u8))))
        })
    }

    pub fn list(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            for arg in args {
                checked_datum(player, symbols, arg)?;
            }
            Ok(player.alloc_datum(Datum::List(DatumType::List, VecDeque::from(args.clone()), false)))
        })
    }

    pub fn image(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            // TODO: Palette ref can be on args[3], need to handle it
            if args.len() < 3 {
                return Err(ScriptError::new(
                    format!("image() expects at least 3 arguments: width, height, bitDepth, optional alphaDepth, got {}", args.len())
                ));
            }

            let width_datum = checked_datum(player, symbols, &args[0])?;
            let height_datum = checked_datum(player, symbols, &args[1])?;

            let width = match width_datum {
                Datum::Int(i) => *i as u16,
                Datum::Float(f) => {
                    let rounded = f.round() as u16;
                    rounded
                }
                _ => {
                    let val = width_datum.int_value()? as u16;
                    val
                }
            };

            let height = match height_datum {
                Datum::Int(i) => *i as u16,
                Datum::Float(f) => {
                    let rounded = f.round() as u16;
                    rounded
                }
                _ => {
                    let val = height_datum.int_value()? as u16;
                    val
                }
            };

            let bit_depth = checked_datum(player, symbols, &args[2])?.int_value()? as u8;
            let mut palette_ref = PaletteRef::BuiltIn(get_system_default_palette());
            let mut alpha_depth = 0;
            if args.len() >= 4 {
                let arg3 = checked_datum(player, symbols, &args[3])?;
                match arg3.type_enum() {
                    DatumType::Int => {
                        alpha_depth = arg3.int_value()? as u8;
                    }
                    DatumType::Symbol => {
                        palette_ref = match arg3 {
                            Datum::Symbol(s) => {
                                PaletteRef::BuiltIn(BuiltInPalette::from_symbol(s.clone(), symbols)?.ok_or_else(|| {
                                    ScriptError::new("image() palette symbol is not a built-in palette".to_string())
                                })?)
                            }
                            _ => {
                                return Err(ScriptError::new(format!(
                                    "Invalid 4th argument type for image(): {}, expected symbol",
                                    arg3.type_str()
                                )))
                            }
                        };
                    }
                    DatumType::PaletteRef => {
                        // If the 4th argument is a palette, then there's no alpha depth specified
                        palette_ref = match arg3 {
                            Datum::PaletteRef(p) => p.clone(),
                            _ => {
                                return Err(ScriptError::new(format!(
                                    "Invalid 4th argument type for image(): {}, expected palette",
                                    arg3.type_str()
                                )))
                            }
                        };
                    }
                    DatumType::CastMemberRef => {
                        // If the 4th argument is a cast member, then there's no alpha depth specified
                        palette_ref = match arg3 {
                            Datum::CastMember(m) => PaletteRef::Member(m.clone()),
                            _ => return Err(ScriptError::new(
                                format!("Invalid 4th argument type for image(): {}, expected int or palette", arg3.type_str())
                            )),
                        };
                    }
                    DatumType::ColorRef => {
                        // Director tolerates rgb(...) as a 4th arg silently; treat as no-op.
                    }
                    _ => {
                        return Err(ScriptError::new(format!(
                            "Invalid 4th argument type for image(): {}, expected int or palette",
                            arg3.type_str()
                        )));
                    }
                }
            }

            // Director's 24-bit images are stored internally as 32-bit RGBA (alpha=0xFF).
            // Both bit_depth and original_bit_depth are set to 32 so all rendering
            // paths (matte, ink, colorize) treat this as a standard 32-bit bitmap.
            let storage_depth = if bit_depth == 24 { 32 } else { bit_depth };
            let mut bitmap = Bitmap::new(
                width,
                height,
                storage_depth,
                storage_depth,
                alpha_depth,
                palette_ref,
            );
            // When image() is created with an alpha channel, treat that
            // embedded alpha as active for subsequent rendering/compositing.
            if storage_depth == 32 && alpha_depth > 0 {
                bitmap.use_alpha = true;
            }
            // `image(w, h, depth)` builds a fresh bitmap not yet owned by any
            // cast member; release it when the wrapping DatumRef drops.
            let bitmap_ref = player.bitmap_manager.add_ephemeral_bitmap(bitmap);
            Ok(player.alloc_datum(Datum::BitmapRef(player.bitmap_handle_for_id(bitmap_ref)?)))
        })
    }

    pub fn abs(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let value = checked_datum(player, symbols, &args[0])?;
            let result = match value {
                Datum::Int(i) => Datum::Int(i.abs()),
                Datum::Float(f) => Datum::Float(f.abs()),
                // Director coerces VOID to 0 in numeric contexts, so
                // `abs(VOID)` is `abs(0)` = 0. Movies routinely read
                // uninitialized properties (e.g. a player's vX/vY before the
                // first physics step in spectral-wizard's colliPlayer) and
                // pass them straight into abs(); Director tolerates this.
                Datum::Void => Datum::Int(0),
                _ => {
                    return Err(ScriptError::new(format!(
                        "Cannot get abs of type: {}",
                        value.type_str()
                    )))
                }
            };
            Ok(player.alloc_datum(result))
        })
    }

    pub fn xtra(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        // `xtra("name")` returns a factory reference; the real
        // registration check happens at `new(xtra "name")` time, so an
        // unknown name doesn't error here. That keeps the door open for
        // on-demand loading (`create_xtra_instance_async`) which would
        // otherwise be short-circuited by this validator. Lingo movies
        // that mis-spell an xtra name surface the error from `new()`
        // instead, with a clearer "Xtra X not found" message.
        runtime.with_player_and_symbols(|player, symbols| {
            // `xtra(xtraNameOrNum)` takes "a string that specifies the name of
            // the Xtra to return, or an integer that specifies the index
            // position of the Xtra to return" (Director 11.5 Scripting
            // Dictionary, `xtra()`). The index form is 1-based and is what
            // `repeat with i = 1 to the number of xtras` walks; without it the
            // integer would fall through to string_value() below and yield a
            // bogus Xtra literally named "3".
            let arg_ref = args.first().ok_or_else(|| ScriptError::new("xtra requires a name or index".to_owned()))?;
            let arg = checked_datum(player, symbols, arg_ref)?.clone();
            if matches!(arg, Datum::Int(_) | Datum::Float(_)) {
                let index = arg.int_value()?;
                let names = get_registered_xtra_names(player);
                if index < 1 || index as usize > names.len() {
                    // "A reference to an empty object is returned if the
                    // specified Xtra is not found" — VOID is that empty
                    // reference here, so `xtra(999).name` reads as VOID
                    // rather than raising mid-loop.
                    return Ok(player.alloc_datum(Datum::Void));
                }
                let name = names[(index - 1) as usize].clone();
                return Ok(player.alloc_datum(Datum::Xtra(name)));
            }

            let xtra_name = arg.string_value(symbols)?;
            let xtra_name = xtra_name.trim().replace(".x32", "");

            // Validate xtra name format: [a-zA-Z0-9_-](\.x32)?
            let is_valid = xtra_name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
            if !is_valid {
                return Err(ScriptError::new(format!(
                    "Invalid xtra name '{}'",
                    xtra_name
                )));
            }
            if !is_xtra_registered(player, &xtra_name) {
                debug!(
                    "Xtra '{}' not yet registered — deferring lookup to new() / on-demand load",
                    xtra_name
                );
            }
            Ok(player.alloc_datum(Datum::Xtra(xtra_name)))
        })
    }

    pub fn union(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            if args.len() != 2 {
                return Err(ScriptError::new("Union requires 2 arguments".to_string()));
            }

            let (left_vals, _lf) = checked_datum(player, symbols, &args[0])?.to_rect_inline()?;
            let (right_vals, _rf) = checked_datum(player, symbols, &args[1])?.to_rect_inline()?;

            let left_tuple = (left_vals[0] as i32, left_vals[1] as i32, left_vals[2] as i32, left_vals[3] as i32);
            let right_tuple = (right_vals[0] as i32, right_vals[1] as i32, right_vals[2] as i32, right_vals[3] as i32);

            let (l, t, r, b) = RectUtils::union(left_tuple, right_tuple);
            let rect = IntRect { left: l, top: t, right: r, bottom: b };

            Ok(player.alloc_datum(rect.to_datum()))
        })
    }

    pub fn bit_xor(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            if args.len() != 2 {
                return Err(ScriptError::new(
                    "Bitwise XOR requires 2 arguments".to_string(),
                ));
            }
            let left = checked_datum(player, symbols, &args[0])?.int_value()?;
            let right = checked_datum(player, symbols, &args[1])?.int_value()?;

            Ok(player.alloc_datum(Datum::Int(left ^ right)))
        })
    }

    /// vector() or vector(x, y, z)
    pub fn vector(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let (x, y, z) = match args.len() {
                0 => (0.0, 0.0, 0.0),
                3 => (
                    checked_datum(player, symbols, &args[0])?.to_float()? as f64,
                    checked_datum(player, symbols, &args[1])?.to_float()? as f64,
                    checked_datum(player, symbols, &args[2])?.to_float()? as f64,
                ),
                _ => {
                    return Err(ScriptError::new(
                        "vector() expects 0 or 3 arguments".to_string(),
                    ))
                }
            };
            Ok(player.alloc_datum(Datum::Vector([x, y, z])))
        })
    }

    pub fn transform3d(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            // transform() with no args returns identity matrix
            if args.is_empty() {
                return Ok(player.alloc_datum(Datum::transform3d([
                    1.0, 0.0, 0.0, 0.0,
                    0.0, 1.0, 0.0, 0.0,
                    0.0, 0.0, 1.0, 0.0,
                    0.0, 0.0, 0.0, 1.0,
                ])));
            }
            Err(ScriptError::new("transform() takes 0 arguments".into()))
        })
    }

    /// `_system.time()` — Director 11.5 Scripting Dictionary, "time() (System)":
    /// "System method; returns the current time in the system clock as a string.
    /// The format of the time string depends on the computer's time settings."
    /// Parameters: None.
    ///
    /// The locale-dependent format is the point of the entry, so defer to the
    /// host's locale (the browser's) rather than hardcoding a US layout — that
    /// is the closest analogue to Director reading the OS time settings.
    pub fn time(runtime: &mut ExecutionContext<'_>, _args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            // Director returns the system SHORT time — hours and minutes only.
            // Measured against Director on this machine: `21:23`, where our
            // bare toLocaleTimeString gave `21:20:15`. Ask for 2-digit hour and
            // minute explicitly and let the host locale decide the separator and
            // 12/24-hour convention, matching the dictionary's note that "the
            // format of the time string depends on the computer's time settings".
            let opts = js_sys::Object::new();
            let _ = js_sys::Reflect::set(&opts, &"hour".into(), &"2-digit".into());
            let _ = js_sys::Reflect::set(&opts, &"minute".into(), &"2-digit".into());
            let mut s = js_sys::Date::new_0()
                .to_locale_time_string_with_options("default", &opts)
                .as_string()
                .unwrap_or_default();
            // Director keeps Windows' short-time layout `HH:mm tt`, so a
            // 24-hour locale leaves the AM/PM slot EMPTY but still emits its
            // separator — measured in Director: `put _system.time()` -> "21:29 "
            // (trailing space), while `put _system.date()` -> "03.08.2026" (none).
            // AreaZero depends on it: its startup line concatenates the two with a
            // plain `&` and no separator of its own.
            //
            // Only 24-hour locales get the space; a 12-hour locale fills that slot
            // with AM/PM, so key off whether the formatted string has a day period.
            // Normalise first: some hosts already emit a trailing separator for the
            // empty slot, and blindly appending produced TWO spaces.
            let has_day_period = s.chars().any(|c| c.is_alphabetic());
            let mut s = s.trim_end().to_string();
            if !has_day_period {
                s.push(' ');
            }
            Ok(player.alloc_datum(Datum::String(s)))
        })
    }

    pub fn date(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let date_id = player.allocator.get_free_script_instance_id();
            let date_obj = if args.len() >= 3 {
                let year = checked_datum(player, symbols, &args[0])?.int_value()?;
                let month = checked_datum(player, symbols, &args[1])?.int_value()?;
                let day = checked_datum(player, symbols, &args[2])?.int_value()?;
                let js_date = js_sys::Date::new_0();
                js_date.set_full_year(year as u32);
                js_date.set_month((month - 1) as u32);
                js_date.set_date(day as u32);
                js_date.set_hours(0);
                js_date.set_minutes(0);
                js_date.set_seconds(0);
                js_date.set_milliseconds(0);
                DateObject::from_timestamp(date_id, js_date.get_time() as i64)
            } else {
                DateObject::new(date_id)
            };
            player.date_objects.insert(date_id, date_obj);
            Ok(player.alloc_datum(Datum::DateRef(date_id)))
        })
    }

    pub fn color(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            match args.len() {
                1 => {
                    // color(paletteIndex) - single argument is palette index
                    let index = checked_datum(player, symbols, &args[0])?.int_value()? as u8;
                    Ok(player.alloc_datum(Datum::ColorRef(ColorRef::PaletteIndex(index))))
                }
                2 => {
                    // color(#rgb, "RRGGBB") or color(#paletteIndex, index)
                    let first = checked_datum(player, symbols, &args[0])?;
                    if let Datum::Symbol(sym) = first {
                        match sym.into_builtin() {
                            Some(BuiltInSymbol::Rgb) => {
                                let hex_str = checked_datum(player, symbols, &args[1])?.string_value(symbols)?.replace("#", "");
                                let r = u8::from_str_radix(&hex_str[0..2], 16).unwrap_or(0);
                                let g = u8::from_str_radix(&hex_str[2..4], 16).unwrap_or(0);
                                let b = u8::from_str_radix(&hex_str[4..6], 16).unwrap_or(0);
                                Ok(player.alloc_datum(Datum::ColorRef(ColorRef::Rgb(r, g, b))))
                            }
                            Some(BuiltInSymbol::PaletteIndex) => {
                                let index = checked_datum(player, symbols, &args[1])?.int_value()? as u8;
                                Ok(player.alloc_datum(Datum::ColorRef(ColorRef::PaletteIndex(index))))
                            }
                            _ => Err(ScriptError::new(format!(
                                "color(): unknown color type symbol #{}",
                                symbols
                                    .display(sym)
                                    .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
                            ))),
                        }
                    } else {
                        Err(ScriptError::new(
                            "color() with 2 arguments expects first argument to be a symbol".to_string(),
                        ))
                    }
                }
                3 => {
                    // color(r, g, b)
                    let r = checked_datum(player, symbols, &args[0])?.int_value()? as u8;
                    let g = checked_datum(player, symbols, &args[1])?.int_value()? as u8;
                    let b = checked_datum(player, symbols, &args[2])?.int_value()? as u8;
                    Ok(player.alloc_datum(Datum::ColorRef(ColorRef::Rgb(r, g, b))))
                }
                4 => {
                    // color(#rgb, r, g, b) - first argument is symbol, skip it
                    let r = checked_datum(player, symbols, &args[1])?.int_value()? as u8;
                    let g = checked_datum(player, symbols, &args[2])?.int_value()? as u8;
                    let b = checked_datum(player, symbols, &args[3])?.int_value()? as u8;
                    Ok(player.alloc_datum(Datum::ColorRef(ColorRef::Rgb(r, g, b))))
                }
                _ => Err(ScriptError::new(format!(
                    "color() expects 1, 2, 3, or 4 arguments, got {}",
                    args.len()
                ))),
            }
        })
    }

    pub fn power(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            if args.len() != 2 {
                return Err(ScriptError::new("Power requires 2 arguments".to_string()));
            }
            let base = checked_datum(player, symbols, &args[0])?;
            let exponent = checked_datum(player, symbols, &args[1])?;

            match (base, exponent) {
                (Datum::Int(base), Datum::Int(exponent)) => {
                    Ok(player.alloc_datum(Datum::Int(base.pow(*exponent as u32))))
                }
                (Datum::Float(base), Datum::Float(exponent)) => {
                    Ok(player.alloc_datum(Datum::Float(base.powf(*exponent))))
                }
                (Datum::Float(base), Datum::Int(exponent)) => {
                    Ok(player.alloc_datum(Datum::Float(base.powf(*exponent as f64))))
                }
                (Datum::Int(base), Datum::Float(exponent)) => {
                    Ok(player.alloc_datum(Datum::Float((*base as f64).powf(*exponent))))
                }
                _ => Err(ScriptError::new("Power requires two numbers".to_string())),
            }
        })
    }

    pub fn add(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        if args.len() != 2 {
            return Err(ScriptError::new("Add requires 2 arguments".to_string()));
        }
        let left_type = checked_datum(&*runtime.player, &*runtime.symbols, &args[0])?.type_enum();
        if left_type == DatumType::Void {
            return Ok(DatumRef::Void);
        }
        match left_type {
            DatumType::List => ListDatumHandlers::add(
                &mut *runtime.player,
                &mut *runtime.symbols,
                args.get(0).unwrap(),
                &vec![args.get(1).unwrap().clone()],
            ),
            _ => Err(ScriptError::new(format!(
                "Add not supported for {}",
                left_type.type_str()
            ))),
        }
    }

    pub fn nothing(_: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        Ok(DatumRef::Void)
    }

    pub fn get_a_prop(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        let datum_ref = args.get(0).unwrap();
        let (datum_type, datum_debug) = runtime.with_player_and_symbols(|player, symbols| -> Result<_, ScriptError> {
            let datum = checked_datum(player, symbols, &args[0])?;
            let debug_str = match datum {
                Datum::Symbol(s) => format!(
                    "#{}",
                    symbols
                        .display(s)
                        .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
                ),
                Datum::String(s) => format!("\"{}\"", s),
                Datum::Int(i) => format!("{}", i),
                _ => format!("{:?}", datum.type_enum()),
            };
            Ok((datum.type_enum(), debug_str))
        })?;
        match datum_type {
            DatumType::PropList => {
                runtime.with_player_and_symbols(|player, symbols| {
                    PropListDatumHandlers::get_a_prop(player, symbols, datum_ref, &vec![args.get(1).unwrap().clone()])
                })
            }
            DatumType::ScriptInstanceRef => {
                get_script_instance_prop_explicit(
                    &mut *runtime.player,
                    &mut *runtime.symbols,
                    datum_ref,
                    &vec![args.get(1).unwrap().clone()],
                )
            }
            // On a LINEAR list, getaProp(list, n) is an indexed access
            // (1-based), equivalent to getAt — Director treats the second arg
            // as a position. hackey/ChannelSurf does
            // `getaProp(getaProp(Whom, i), #z)`: the inner call indexes the
            // list-of-proplists by integer, the outer looks up #z.
            DatumType::List => {
                runtime.with_player_and_symbols(|player, symbols| {
                    ListDatumHandlers::get_at(player, symbols, datum_ref, &vec![args.get(1).unwrap().clone()])
                })
            }
            _ => Err(ScriptError::new(format!(
                "Cannot getaProp prop of type: {} (value: {})",
                datum_type.type_str(),
                datum_debug
            ))),
        }
    }

    pub fn min(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            if args.len() == 0 {
                return Ok(player.alloc_datum(Datum::Int(0)));
            }
            let args_vec;
            let args = if checked_datum(player, symbols, &args[0])?.is_list() {
                args_vec = Vec::from(checked_datum(player, symbols, &args[0])?.to_list()?.clone());
                &args_vec
            } else {
                args
            };
            if args.len() == 0 {
                // TODO this returns [] instead
                return Ok(player.alloc_datum(Datum::Int(0)));
            }

            let sorted_list = sort_datums(args, &player.allocator, symbols)?;
            return Ok(sorted_list.first().unwrap().clone());
        })
    }

    pub fn max(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            if args.len() == 0 {
                return Ok(player.alloc_datum(Datum::Int(0)));
            }
            let args_vec;
            let args = if checked_datum(player, symbols, &args[0])?.is_list() {
                args_vec = Vec::from(checked_datum(player, symbols, &args[0])?.to_list()?.clone());
                &args_vec
            } else {
                args
            };
            if args.len() == 0 {
                // TODO this returns [] instead
                return Ok(player.alloc_datum(Datum::Int(0)));
            }

            let sorted_list = sort_datums(args, &player.allocator, symbols)?;
            return Ok(sorted_list.last().unwrap().clone());
        })
    }

    pub fn sort(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let datum_ref = &args[0];
            match checked_datum(player, symbols, datum_ref)? {
                Datum::PropList(_, _) => PropListDatumHandlers::sort(player, symbols, datum_ref, &vec![]),
                Datum::List(_, _, _) => ListDatumHandlers::sort(player, symbols, datum_ref, &vec![]),
                _ => Ok(DatumRef::Void),
            }
        })
    }

    pub fn intersect(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            if args.len() != 2 {
                return Err(ScriptError::new("Intersect requires 2 arguments".to_string()));
            }

            let (left_vals, _lf) = checked_datum(player, symbols, &args[0])?.to_rect_inline()?;
            let (right_vals, _rf) = checked_datum(player, symbols, &args[1])?.to_rect_inline()?;

            let left = (left_vals[0] as i32, left_vals[1] as i32, left_vals[2] as i32, left_vals[3] as i32);
            let right = (right_vals[0] as i32, right_vals[1] as i32, right_vals[2] as i32, right_vals[3] as i32);

            let (l, t, r, b) = RectUtils::intersect(left, right);
            let rect = IntRect { left: l, top: t, right: r, bottom: b };

            Ok(player.alloc_datum(rect.to_datum()))
        })
    }

    /// `map(targetRect, sourceRect, destinationRect)` or
    /// `map(targetPoint, sourceRect, destinationRect)` — "positions and sizes a
    /// rectangle or point based on the relationship of a source rectangle to a
    /// target rectangle. The relationship of the targetRect to the sourceRect
    /// governs the relationship of the result of the function to the
    /// destinationRect" (Director 11.5 Scripting Dictionary, `map()`).
    ///
    /// So each component is rescaled proportionally out of the source rect and
    /// into the destination rect. All three arguments are documented as
    /// Required.
    ///
    /// The dictionary doesn't state the numeric type of the result. Director's
    /// rects and points hold floats, but `map()` predates that, so we keep the
    /// result integral unless one of the inputs was itself fractional — that
    /// promotion rule is inferred, not specified. A degenerate source axis
    /// (zero width or height) would divide by zero; we fall back to a scale of
    /// 1 on that axis, which is also inferred.
    pub fn map(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            if args.len() != 3 {
                return Err(ScriptError::new(
                    "map requires 3 arguments (targetRect/targetPoint, sourceRect, destinationRect)"
                        .to_string(),
                ));
            }

            let (src, src_flags) = checked_datum(player, symbols, &args[1])?.to_rect_inline()?;
            let (dst, dst_flags) = checked_datum(player, symbols, &args[2])?.to_rect_inline()?;

            let scale = |s_lo: f64, s_hi: f64, d_lo: f64, d_hi: f64| {
                let span = s_hi - s_lo;
                if span == 0.0 { 1.0 } else { (d_hi - d_lo) / span }
            };
            let sx = scale(src[0], src[2], dst[0], dst[2]);
            let sy = scale(src[1], src[3], dst[1], dst[3]);
            let map_x = |x: f64| dst[0] + (x - src[0]) * sx;
            let map_y = |y: f64| dst[1] + (y - src[1]) * sy;

            let target = checked_datum(player, symbols, &args[0])?;
            let (mapped, target_flags, is_rect): (Vec<f64>, u8, bool) = match target {
                Datum::Rect(vals, flags) => (
                    vec![map_x(vals[0]), map_y(vals[1]), map_x(vals[2]), map_y(vals[3])],
                    *flags,
                    true,
                ),
                Datum::Point(vals, flags) => {
                    (vec![map_x(vals[0]), map_y(vals[1])], *flags, false)
                }
                _ => {
                    return Err(ScriptError::new(
                        "map expects a rect or a point as its first argument".to_string(),
                    ))
                }
            };

            // Any fractional input keeps the result fractional; otherwise round
            // back to whole numbers (Director coerces floats to ints by
            // rounding, not truncating).
            let fractional = target_flags != 0 || src_flags != 0 || dst_flags != 0;
            let component = |v: f64| if fractional { v } else { v.round() };

            let datum = if is_rect {
                let flags = if fractional { 0b1111 } else { 0 };
                Datum::Rect(
                    [
                        component(mapped[0]),
                        component(mapped[1]),
                        component(mapped[2]),
                        component(mapped[3]),
                    ],
                    flags,
                )
            } else {
                let flags = if fractional { 0b11 } else { 0 };
                Datum::Point([component(mapped[0]), component(mapped[1])], flags)
            };

            Ok(player.alloc_datum(datum))
        })
    }

    /// `inflate(rect, widthChange, heightChange)` — expands (or, with
    /// negatives, shrinks) a rect by `widthChange` on the left and right and
    /// `heightChange` on the top and bottom: left/top decrease, right/bottom
    /// increase, so total width grows by 2*widthChange. Returns a new rect.
    ///
    /// NOTE: `inflate` is NOT in the bundled Director 11.5 Scripting Dictionary
    /// (verified via the director-reference skill — the dict documents only
    /// `rect()`, `map()`, `union()`, `intersect()`). This is the standard
    /// Director rect-inflate contract, inferred from documented behavior and
    /// the MM custom scroll bar's InstallElement, which pads the dragger active
    /// zone with `inflate(zone, 32, 32)` (an outward expansion).
    pub fn inflate(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            if args.len() != 3 {
                return Err(ScriptError::new(
                    "inflate requires 3 arguments (rect, widthChange, heightChange)".to_string(),
                ));
            }
            let (vals, _f) = checked_datum(player, symbols, &args[0])?.to_rect_inline()?;
            let dw = checked_datum(player, symbols, &args[1])?.int_value()?;
            let dh = checked_datum(player, symbols, &args[2])?.int_value()?;
            let rect = IntRect {
                left: vals[0] as i32 - dw,
                top: vals[1] as i32 - dh,
                right: vals[2] as i32 + dw,
                bottom: vals[3] as i32 + dh,
            };
            Ok(player.alloc_datum(rect.to_datum()))
        })
    }

    /// `pointToChar(spriteRef, point)` (also `sprite.pointToChar(point)`) —
    /// Director 11.5 Scripting Dictionary: "returns an integer representing the
    /// character position located within the text or field sprite at a
    /// specified screen coordinate, or returns -1 if the point is not within
    /// the text." 1-based. Shares the hit-test core with `the mouseChar`.
    /// Used by the customHyperlink behavior to find the char under the mouse.
    pub fn point_to_char(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            if args.len() != 2 {
                return Err(ScriptError::new(
                    "pointToChar requires 2 arguments (sprite, point)".to_string(),
                ));
            }
            let sprite_num = checked_datum(player, symbols, &args[0])?.to_sprite_ref()?;
            let (pt_vals, _f) = checked_datum(player, symbols, &args[1])?.to_point_inline()?;
            let result =
                crate::player::compute_char_at(player, sprite_num, pt_vals[0] as i32, pt_vals[1] as i32);
            Ok(player.alloc_datum(Datum::Int(result)))
        })
    }

    /// `scrollByLine(member, amount)` — global form of `member.scrollByLine()`
    /// (Director 11.5 Scripting Dictionary p.618). Scrolls a field/text member
    /// by `amount` lines (positive = down). The MM custom scroll bar calls this
    /// global form from its `move`/`MoveBar` handlers.
    pub fn scroll_by_line(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            if args.len() != 2 {
                return Err(ScriptError::new(
                    "scrollByLine requires 2 arguments (member, amount)".to_string(),
                ));
            }
            let member_ref = checked_datum(player, symbols, &args[0])?.to_member_ref()?;
            let amount = checked_datum(player, symbols, &args[1])?.to_float()?;
            crate::player::handlers::datum_handlers::cast_member::text::scroll_member_by_lines(
                player, &member_ref, amount,
            );
            Ok(DatumRef::Void)
        })
    }

    /// `scrollByPage(member, amount)` — global form of `member.scrollByPage()`
    /// (Director 11.5 Scripting Dictionary p.619). Scrolls a field/text member
    /// by `amount` pages (a page = the lines visible in the member's box).
    pub fn scroll_by_page(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            if args.len() != 2 {
                return Err(ScriptError::new(
                    "scrollByPage requires 2 arguments (member, amount)".to_string(),
                ));
            }
            let member_ref = checked_datum(player, symbols, &args[0])?.to_member_ref()?;
            let amount = checked_datum(player, symbols, &args[1])?.to_float()?;
            crate::player::handlers::datum_handlers::cast_member::text::scroll_member_by_pages(
                player, &member_ref, amount,
            );
            Ok(DatumRef::Void)
        })
    }

    // pub fn get_prop_at(args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
    //     let datum_ref = args.get(0).unwrap();
    //     let prop_key_ref = args.get(1).unwrap();
    //     reserve_player_mut(|player| TypeUtils::get_sub_prop(datum_ref, prop_key_ref, player))
    // }

    pub fn get_prop_at(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        use crate::player::datum_formatting::format_concrete_datum;
        runtime.with_player_and_symbols(|player, symbols| {
            let prop_list_ref = &args[0];
            let position = checked_datum(player, symbols, &args[1])?.int_value()?;
            let index = (position - 1) as usize;
            
            let prop_list = checked_datum(player, symbols, prop_list_ref)?;
            
            debug!(
                "🔍 getPropAt: proplist={}, index={}", 
                format_concrete_datum(prop_list, symbols, player)?,
                position
            );
            
            match prop_list {
                Datum::PropList(entries, _) => {
                    if index >= entries.len() {
                        return Err(ScriptError::new(format!(
                            "Index {} out of bounds for proplist of length {}", 
                            position, 
                            entries.len()
                        )));
                    }
                    // Return the KEY at this position, not the value!
                    let key_ref = entries[index].0.clone();
                    
                    debug!(
                        "✅ getPropAt returned key: {}", 
                        format_concrete_datum(checked_datum(player, symbols, &key_ref)?, symbols, player)?
                    );
                    
                    Ok(key_ref)
                }
                _ => Err(ScriptError::new(
                    "getPropAt requires a property list".to_string()
                )),
            }
        })
    }

    pub fn pi(runtime: &mut ExecutionContext<'_>, _: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        Ok(runtime.player.alloc_datum(Datum::Float(std::f64::consts::PI)))
    }

    pub fn sin(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let value = checked_datum(player, symbols, &args[0])?.to_float()?;
            Ok(player.alloc_datum(Datum::Float(value.sin())))
        })
    }

    pub fn cos(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let value = checked_datum(player, symbols, &args[0])?.to_float()?;
            Ok(player.alloc_datum(Datum::Float(value.cos())))
        })
    }

    pub fn sqrt(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let value = checked_datum(player, symbols, &args[0])?;
            
            let num = if let Ok(f) = value.float_value() {
                f
            } else if let Ok(i) = value.int_value() {
                i as f64
            } else {
                return Err(ScriptError::new("sqrt requires a number".to_string()));
            };
            
            if num < 0.0 {
                return Err(ScriptError::new("sqrt of negative number".to_string()));
            }
            
            let result = num.sqrt();
            Ok(player.alloc_datum(Datum::Float(result)))
        })
    }

    pub fn tan(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let value = checked_datum(player, symbols, &args[0])?.to_float()?;
            Ok(player.alloc_datum(Datum::Float(value.tan())))
        })
    }

    pub fn atan(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let num = |dr: &DatumRef| -> Result<f64, ScriptError> {
                let value = checked_datum(player, symbols, dr)?;
                if let Ok(f) = value.float_value() {
                    Ok(f)
                } else if let Ok(i) = value.int_value() {
                    Ok(i as f64)
                } else {
                    Err(ScriptError::new("atan requires a number".to_string()))
                }
            };
            let a0 = num(&args[0])?;
            // Two-arg `atan(y, x)` is arctangent-of-two-values (atan2). Director's
            // built-in atan is single-arg, but the 3D Groove Xtra exports a 2-arg
            // `atan` (RE: `atan(a1,a2) = atan2(a2,a1)`) that movies call to aim one
            // object at another (e.g. BioBoxing's `atan(dx, dy)` enemy heading).
            // Built-ins resolve before xtra commands, so handle the 2-arg form here.
            //
            // Heatwave Racing (a plain Director movie, no Groove) also relies on
            // the 2-arg form: its car physics computes the downhill push angle as
            // `atan(tiltvector.y, tiltvector.x)`. Restricting this to single-arg
            // per the Scripting Dictionary was tried and made the cars far worse
            // (they slid off the banking and stalled), so atan2 stays unconditional.
            let result = if args.len() >= 2 {
                a0.atan2(num(&args[1])?)
            } else {
                a0.atan()
            };
            Ok(player.alloc_datum(Datum::Float(result)))
        })
    }

    pub fn sound(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            // Bare `sound` with no arguments is a harmless no-op in Director, not
            // a crash. Dora Soccer's timeoptions queue can leave `thingtodo =
            // "sound"` and later run `do("sound")`; indexing args[0] here panicked
            // (index out of bounds) and took down the whole VM.
            if args.is_empty() {
                return Ok(DatumRef::Void);
            }
            let first_arg = player.get_datum(&args[0]).clone();
            // Command form: sound(#verb, channelNum, ...args)
            // e.g. sound #stop, 3  or  sound #play, 1, member("snd")
            if let Datum::Symbol(verb) = &first_arg {
                let verb = verb.clone();
                let channel_num = if args.len() > 1 {
                    player.get_datum(&args[1]).int_value()? as u16
                } else {
                    1 // default to channel 1
                };
                if channel_num == 0 || channel_num as usize > player.sound_manager.num_channels() {
                    return Err(ScriptError::new(format!(
                        "Invalid sound channel: {}",
                        channel_num
                    )));
                }
                let channel_datum = player.alloc_datum(Datum::SoundChannel(channel_num));
                let remaining_args: Vec<DatumRef> = args[2..].to_vec();
                SoundChannelDatumHandlers::call(player, symbols, &channel_datum, verb, &remaining_args)
            } else {
                // Function form: sound(channelNum) - returns a SoundChannel datum
                let channel_num = first_arg.int_value()? as u16;
                if channel_num == 0 || channel_num as usize > player.sound_manager.num_channels() {
                    return Err(ScriptError::new(format!(
                        "Invalid sound channel: {}",
                        channel_num
                    )));
                }
                Ok(player.alloc_datum(Datum::SoundChannel(channel_num)))
            }
        })
    }

    /// Resolve every `callAncestor` target while the owner is borrowed. The
    /// returned child list is ordered exactly like the legacy loop; the caller
    /// executes one child at a time and retains the last result.
    pub fn prepare_call_ancestor(
        runtime: &mut ExecutionContext<'_>,
        args: &[DatumRef],
    ) -> Result<AncestorCallPlan, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            if args.len() < 2 {
                return Err(ScriptError::new(
                    "callAncestor requires a handler and instance".to_owned(),
                ));
            }
            let handler_name = checked_datum(player, symbols, &args[0])?.symbol_value(symbols)?;
            let current_scope_ref = player.current_scope_ref();
            let current_script_ref = player.scopes.get(current_scope_ref)
                .map(|scope| scope.script_ref.clone());
            let instance_list = match checked_datum(player, symbols, &args[1])? {
                Datum::List(_, list, _) => list.clone(),
                Datum::ScriptInstanceRef(_) => VecDeque::from([args[1].clone()]),
                _ => return Err(ScriptError::new(
                    "Can only callAncestor on script instances and lists".to_owned(),
                )),
            };
            let extra_args = args[2..].to_vec();
            let mut calls = Vec::new();
            for instance_datum_ref in instance_list {
                let original_me_ref = match checked_datum(player, symbols, &instance_datum_ref)? {
                    Datum::ScriptInstanceRef(reference) => reference.clone(),
                    _ => return Err(ScriptError::new(
                        "callAncestor list contains a non-script instance".to_owned(),
                    )),
                };
                let mut ancestor_source = original_me_ref.clone();
                if let Some(current_script_ref) = &current_script_ref {
                    let mut walk = original_me_ref.clone();
                    for _ in 0..100 {
                        let instance = player.allocator.get_script_instance_opt(&walk)
                            .ok_or_else(|| ScriptError::new_code(
                                ScriptErrorCode::InvalidReference,
                                "stale callAncestor instance".to_owned(),
                            ))?;
                        if instance.script == *current_script_ref {
                            ancestor_source = walk;
                            break;
                        }
                        let Some(next) = instance.ancestor.clone() else { break };
                        walk = next;
                    }
                }
                let source = player.allocator.get_script_instance_opt(&ancestor_source)
                    .ok_or_else(|| ScriptError::new_code(
                        ScriptErrorCode::InvalidReference,
                        "stale callAncestor instance".to_owned(),
                    ))?;
                let Some(walk) = source.ancestor.clone() else {
                    return Ok(AncestorCallPlan::Complete(DatumRef::Void));
                };
                let mut child_args = Vec::with_capacity(extra_args.len() + 1);
                child_args.push(instance_datum_ref);
                child_args.extend(extra_args.iter().cloned());
                calls.push(AncestorCall {
                    receiver: original_me_ref,
                    source: walk,
                    handler_name: handler_name.clone(),
                    args: child_args,
                });
            }
            if calls.is_empty() {
                Ok(AncestorCallPlan::Complete(DatumRef::Void))
            } else {
                Ok(AncestorCallPlan::Children(calls))
            }
        })
    }

    pub fn new_object(
        runtime: &mut ExecutionContext<'_>,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        if args.is_empty() {
            return Err(ScriptError::new(
                "newObject requires at least one argument".to_owned(),
            ));
        }
        runtime.with_player_and_symbols(|player, symbols| {
            let object_type = checked_datum(player, symbols, &args[0])?
                .string_value(symbols)?;
            match object_type.to_lowercase().as_str() {
                "xml" => {
                    let xml_id = player.next_xml_id;
                    player.next_xml_id += 1;
                    player.xml_documents.insert(xml_id, XmlDocument {
                        id: xml_id,
                        root_element: None,
                        content: String::new(),
                        ignore_white: false,
                    });
                    Ok(player.alloc_datum(Datum::XmlRef(xml_id)))
                }
                "date" => {
                    let date_id = player.allocator.get_free_script_instance_id();
                    let date_obj = if args.len() >= 4 {
                        let year = checked_datum(player, symbols, &args[1])?.int_value()?;
                        let month = checked_datum(player, symbols, &args[2])?.int_value()?;
                        let day = checked_datum(player, symbols, &args[3])?.int_value()?;
                        let js_date = js_sys::Date::new_0();
                        js_date.set_full_year(year as u32);
                        js_date.set_month((month - 1) as u32);
                        js_date.set_date(day as u32);
                        js_date.set_hours(0);
                        js_date.set_minutes(0);
                        js_date.set_seconds(0);
                        js_date.set_milliseconds(0);
                        DateObject::from_timestamp(date_id, js_date.get_time() as i64)
                    } else {
                        DateObject::new(date_id)
                    };
                    player.date_objects.insert(date_id, date_obj);
                    Ok(player.alloc_datum(Datum::DateRef(date_id)))
                }
                "math" => {
                    let math_id = player.allocator.get_free_script_instance_id();
                    player.math_objects.insert(math_id, MathObject::new(math_id));
                    Ok(player.alloc_datum(Datum::MathRef(math_id)))
                }
                "object" => Ok(player.alloc_datum(Datum::PropList(VecDeque::new(), false))),
                "string" => {
                    let value = args.get(1)
                        .map(|value| checked_datum(player, symbols, value)?.string_value(symbols))
                        .transpose()?
                        .unwrap_or_default();
                    Ok(player.alloc_datum(Datum::String(value)))
                }
                "array" => Ok(player.alloc_datum(Datum::List(
                    DatumType::XmlChildNodes,
                    VecDeque::new(),
                    false,
                ))),
                _ => Err(ScriptError::new(format!(
                    "newObject: Unsupported object type '{}'",
                    object_type
                ))),
            }
        })
    }

    pub fn sound_busy(
        runtime: &mut ExecutionContext<'_>,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        runtime.with_player(|player| {
            if args.is_empty() {
                return Err(ScriptError::new("soundBusy requires a channel number".to_string()));
            }
            
            let channel_num_ref = &args[0];
            let channel_num = player.get_datum(channel_num_ref).int_value()?;
            
            // Convert to 0-based index
            let channel_idx = (channel_num - 1) as usize;

            #[cfg(not(target_arch = "wasm32"))]
            {
                let is_busy = player.sound_manager.native_sound_busy(channel_idx)?;
                return Ok(player.alloc_datum(Datum::Int(if is_busy { 1 } else { 0 })));
            }

            #[cfg(target_arch = "wasm32")]
            {
            
            // Get the channel directly from sound manager
            let channel_rc = player.sound_manager.get_channel(channel_idx)
                .ok_or_else(|| ScriptError::new(format!(
                    "Sound channel {} out of range",
                    channel_num
                )))?;
            
            let channel = channel_rc.borrow();
            
            // soundBusy returns true (1) if the channel is Playing, Loading, or Queued
            let status = channel.status.clone();
            let has_source = channel.source_node.is_some();
            let sample_rate = channel.sample_rate;
            let sample_count = channel.sample_count;
            let elapsed_time = channel.elapsed_time;
            let ctx_time = channel.context_time();
            let start_time = channel.playback_start_context_time;
            let loop_count = channel.loop_count;

            // Check buffer duration for the Playing+source case
            let buffer_duration = if status == SoundStatus::Playing {
                channel.source_node.as_ref()
                    .and_then(|s| s.buffer())
                    .map(|b| b.duration())
            } else {
                None
            };

            // Drop the immutable borrow so we can mutably borrow if needed
            drop(channel);

            let mut is_busy = matches!(
                status,
                SoundStatus::Playing | SoundStatus::Loading | SoundStatus::Queued
            );

            if is_busy {
                debug!(
                    "🔍 soundBusy({}) = true, status={:?}, has_source={}, sample_rate={}, sample_count={}, elapsed={:.3}, ctx_time={:.3}, start_time={:.3}, buf_dur={:?}",
                    channel_num, status, has_source, sample_rate, sample_count,
                    elapsed_time, ctx_time, start_time, buffer_duration,
                );
            }

            // Check if sound has actually finished by comparing AudioContext time
            // against the source node's buffer duration.
            if is_busy && status == SoundStatus::Playing {
                if let Some(duration) = buffer_duration {
                    let elapsed = ctx_time - start_time;
                    if elapsed > duration && loop_count != 0 {
                        debug!(
                            "⏱️ soundBusy({}) forcing Idle: elapsed={:.3} > duration={:.3}",
                            channel_num, elapsed, duration
                        );
                        is_busy = false;
                        let mut ch = channel_rc.borrow_mut();
                        ch.status = SoundStatus::Idle;
                        ch.source_node = None;
                    }
                } else if !has_source {
                    // Playing but no source node - stuck state, force idle
                    debug!(
                        "⚠️ soundBusy({}) Playing with no source_node, forcing Idle",
                        channel_num
                    );
                    is_busy = false;
                    channel_rc.borrow_mut().status = SoundStatus::Idle;
                }
            }

            // Also catch Loading state that's been stuck too long (>5 seconds)
            if is_busy && status == SoundStatus::Loading {
                if start_time > 0.0 && ctx_time - start_time > 5.0 {
                    debug!(
                        "⚠️ soundBusy({}) stuck in Loading for {:.1}s, forcing Idle",
                        channel_num, ctx_time - start_time
                    );
                    is_busy = false;
                    channel_rc.borrow_mut().status = SoundStatus::Idle;
                }
            }

            Ok(player.alloc_datum(Datum::Int(if is_busy { 1 } else { 0 })))
            }
        })
    }
}

/// Director's `value()` evaluates only the FIRST complete expression in the
/// string. When that expression is a list or property list, trailing tokens
/// after the matching close bracket are ignored. If `s` (ignoring leading
/// whitespace) starts with `[`, return the slice up to and including the
/// bracket that balances it; otherwise return `s` unchanged.
///
/// Bracket scanning is string-aware: `[`/`]` inside a Lingo string literal
/// (`"..."`) don't count. Lingo string literals cannot contain a literal
/// double quote (the `QUOTE` constant is used instead), so a simple in-string
/// toggle is sufficient. If the brackets never balance, `s` is returned as-is
/// so the normal parser still reports the error.
pub(crate) fn truncate_to_first_balanced_list(s: &str) -> String {
    let trimmed_start = s.len() - s.trim_start().len();
    let bytes = s.as_bytes();
    if bytes.get(trimmed_start) != Some(&b'[') {
        return s.to_string();
    }
    let mut depth: i32 = 0;
    let mut in_string = false;
    for (i, &b) in bytes.iter().enumerate().skip(trimmed_start) {
        match b {
            b'"' => in_string = !in_string,
            b'[' if !in_string => depth += 1,
            b']' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    // Include this closing bracket; drop everything after.
                    return s[..=i].to_string();
                }
            }
            _ => {}
        }
    }
    s.to_string()
}

/// Returns true if the given input/cleaned strings look like a Lingo
/// "fragment retry" — i.e. a partial piece produced by code that splits a
/// prop list at `]` characters (e.g. Coke Studios' ElementManager.parsewindow
/// feeding `sElement = sElements.item[i] & "]"` to value() and relying on
/// Void on failure to drive a retry loop). These fragments legitimately
/// fail to parse on the first attempt and we don't want to warn on them.
pub fn is_expected_value_retry_fragment(input: &str, cleaned: &str) -> bool {
    let t = input.trim();
    // Trivially just a close bracket
    if t == "]" {
        return true;
    }
    // Tail fragment after a split — starts with a property-list separator
    if t.starts_with(',') || t.starts_with(':') {
        return true;
    }
    // Opened bracket with no close — normalisation stripped it to nothing
    if cleaned.is_empty() && (t.starts_with('[') || t.starts_with('(')) {
        return true;
    }
    false
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod ownership_tests {
    use super::TypeHandlers;
    use crate::director::lingo::datum::{Datum, DatumType};
    use crate::player::session::RuntimeSession;
    use crate::player::symbols::{builtin::BuiltInSymbol, symbol_table::SymbolOwner};
    use crate::player::{DatumRef, ScriptErrorCode};
    use async_std::channel;
    use std::collections::VecDeque;

    fn session() -> RuntimeSession {
        let mut session = RuntimeSession::new(SymbolOwner { session: 601, generation: 1 });
        let (tx, _rx) = channel::unbounded();
        assert!(session.add_player(1, tx));
        let (tx, _rx) = channel::unbounded();
        assert!(session.add_player(2, tx));
        session
    }

    fn int(session: &mut RuntimeSession, player_id: u32, value: i32) -> DatumRef {
        session
            .with_player(player_id, |ctx| ctx.player.alloc_datum(Datum::Int(value)))
            .unwrap()
    }

    #[test]
    fn new_matrix_keeps_local_fallback_but_rejects_foreign_consumed_cell() {
        let mut local = session();
        let nested = local.with_player(1, |ctx| {
            let text = ctx.player.alloc_datum(Datum::String("unsupported".to_owned()));
            ctx.player.alloc_datum(Datum::List(
                DatumType::List,
                VecDeque::from([text]),
                false,
            ))
        }).unwrap();
        let initial = local.with_player(1, |ctx| {
            ctx.player.alloc_datum(Datum::List(
                DatumType::List,
                VecDeque::from([nested]),
                false,
            ))
        }).unwrap();
        let result = local.with_player(1, |mut ctx| {
            let rows = ctx.player.alloc_datum(Datum::Int(1));
            let cols = ctx.player.alloc_datum(Datum::Int(1));
            TypeHandlers::new_matrix(&mut ctx, &vec![rows, cols, initial])
        }).unwrap().unwrap();
        local.with_player(1, |ctx| {
            let matrix = ctx.player.get_datum(&result);
            let (_, rows, _) = matrix.to_list_tuple().unwrap();
            let (_, cells, _) = ctx.player.get_datum(&rows[0]).to_list_tuple().unwrap();
            assert!(matches!(ctx.player.get_datum(&cells[0]), Datum::Int(0)));
        }).unwrap();

        let mut foreign = session();
        let foreign_value = int(&mut foreign, 2, 9);
        let initial = foreign.with_player(1, |ctx| {
            ctx.player.alloc_datum(Datum::List(
                DatumType::List,
                VecDeque::from([foreign_value]),
                false,
            ))
        }).unwrap();
        let error = foreign.with_player(1, |mut ctx| {
            let rows = ctx.player.alloc_datum(Datum::Int(1));
            let cols = ctx.player.alloc_datum(Datum::Int(1));
            TypeHandlers::new_matrix(&mut ctx, &vec![rows, cols, initial]).unwrap_err()
        }).unwrap();
        assert_eq!(error.code, ScriptErrorCode::InvalidReference);
    }

    #[test]
    fn list_rejects_foreign_retained_argument() {
        let mut session = session();
        let first = int(&mut session, 1, 3);
        let second = int(&mut session, 1, 4);
        let ordered = session.with_player(1, |mut ctx| {
            TypeHandlers::list(&mut ctx, &vec![first.clone(), second.clone()]).unwrap()
        }).unwrap();
        session.with_player(1, |ctx| {
            let (_, items, _) = ctx.player.get_datum(&ordered).to_list_tuple().unwrap();
            assert_eq!(items[0], first);
            assert_eq!(items[1], second);
        }).unwrap();

        let foreign = int(&mut session, 2, 17);
        let error = session.with_player(1, |mut ctx| {
            TypeHandlers::list(&mut ctx, &vec![foreign]).unwrap_err()
        }).unwrap();
        assert_eq!(error.code, ScriptErrorCode::InvalidReference);
    }

    #[test]
    fn filter_ignores_local_non_property_second_argument() {
        let mut session = session();
        let kind = session.with_player(1, |ctx| {
            ctx.player.alloc_datum(Datum::Symbol(crate::player::symbols::symbol::Symbol::builtin(
                BuiltInSymbol::AdjustColorFilter,
            )))
        }).unwrap();
        let ignored = int(&mut session, 1, 123);
        let extra_ignored = int(&mut session, 2, 456);
        let result = session.with_player(1, |mut ctx| {
            TypeHandlers::filter(&mut ctx, &vec![kind, ignored, extra_ignored]).unwrap()
        }).unwrap();
        session.with_player(1, |ctx| {
            match ctx.player.get_datum(&result) {
                Datum::PropList(props, _) => assert_eq!(props.len(), 1),
                other => panic!("filter returned {}", other.type_str()),
            }
        }).unwrap();
    }

    #[test]
    fn filter_rejects_foreign_retained_property_entry() {
        let mut session = session();
        let kind = session.with_player(1, |ctx| {
            ctx.player.alloc_datum(Datum::Symbol(crate::player::symbols::symbol::Symbol::builtin(
                BuiltInSymbol::AdjustColorFilter,
            )))
        }).unwrap();
        let mut foreign = RuntimeSession::new(SymbolOwner { session: 602, generation: 1 });
        let (tx, _rx) = channel::unbounded();
        assert!(foreign.add_player(1, tx));
        let (foreign_key, foreign_value) = foreign.with_player(1, |ctx| {
            let key = ctx.player.alloc_datum(Datum::Symbol(ctx.symbols.intern("foreignKey")));
            let value = ctx.player.alloc_datum(Datum::Int(42));
            (key, value)
        }).unwrap();
        let retained = session.with_player(1, |ctx| {
            ctx.player.alloc_datum(Datum::PropList(
                VecDeque::from([(foreign_key, foreign_value.clone())]),
                false,
            ))
        }).unwrap();
        let error = session.with_player(1, |mut ctx| {
            TypeHandlers::filter(&mut ctx, &vec![kind.clone(), retained]).unwrap_err()
        }).unwrap();
        assert_eq!(error.code, ScriptErrorCode::InvalidReference);

        // A local key must not mask a foreign value retained by the
        // property-list argument. The key and value are consumed separately
        // by filter, so both ownership directions need coverage.
        let local_key = session.with_player(1, |ctx| {
            ctx.player.alloc_datum(Datum::Symbol(ctx.symbols.intern("localKey")))
        }).unwrap();
        let retained_value = session.with_player(1, |ctx| {
            ctx.player.alloc_datum(Datum::PropList(
                VecDeque::from([(local_key, foreign_value)]),
                false,
            ))
        }).unwrap();
        let error = session.with_player(1, |mut ctx| {
            TypeHandlers::filter(&mut ctx, &vec![kind, retained_value]).unwrap_err()
        }).unwrap();
        assert_eq!(error.code, ScriptErrorCode::InvalidReference);
    }
}
