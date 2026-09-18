use std::sync::Arc;

use log::{debug, warn};

use crate::{
    director::lingo::datum::Datum,
    player::{
        bitmap::{manager::BitmapId, mask::BitmapMask},
        datum_formatting::format_concrete_datum,
        score::sprite_get_prop,
        symbols::{builtin::BuiltInSymbol, symbol::Symbol, symbol_table::SymbolTable},
        DatumRef, DirPlayer, HandlerExecutionResult, ScriptError,
    },
};

use super::handler_manager::BytecodeHandlerContext;

/// Check if an ink value requires matte (pixel-level) collision
fn is_matte_ink(ink: i32) -> bool {
    ink == 8 || ink == 36 || ink == 33 || ink == 41 || ink == 7
}

fn diagnostic_datum(datum: &Datum, symbols: &SymbolTable, player: &DirPlayer) -> String {
    format_concrete_datum(datum, symbols, player)
        .unwrap_or_else(|_| format!("<unformattable {}>", datum.type_str()))
}

fn checked_sprite_operand<'a>(
    player: &'a DirPlayer,
    symbols: &SymbolTable,
    datum_ref: &DatumRef,
) -> Result<&'a Datum, ScriptError> {
    let datum = match datum_ref {
        DatumRef::Void => return Ok(&Datum::Void),
        _ => player
            .allocator
            .try_get_datum(datum_ref)
            .ok_or_else(|| ScriptError::new("invalid or foreign sprite reference".to_owned()))?,
    };
    if let Datum::Symbol(symbol) = datum {
        symbols
            .display(symbol)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
    }
    Ok(datum)
}

/// Get the bitmap image_ref for a sprite's cast member (if it's a bitmap).
/// Returns (image_ref, bitmap_width, bitmap_height).
fn get_sprite_image_ref(player: &DirPlayer, sprite_num: i16) -> Option<(BitmapId, u16, u16)> {
    let member_ref = player
        .movie
        .score
        .get_channel(sprite_num)
        .sprite
        .member
        .as_ref()?
        .clone();
    let member = player.movie.cast_manager.find_member_by_ref(&member_ref)?;
    let bmp = member.member_type.as_bitmap()?;
    let bitmap = player.bitmap_manager.get_bitmap(bmp.image_ref)?;
    Some((bmp.image_ref, bitmap.width, bitmap.height))
}

/// Check pixel-level collision between two sprites using their bitmap mattes.
/// Only called when AABB already overlaps and at least one sprite has matte ink.
fn check_matte_pixel_overlap(
    player: &DirPlayer,
    src_num: i16,
    tgt_num: i16,
    src_rect: (i32, i32, i32, i32),
    tgt_rect: (i32, i32, i32, i32),
    src_is_matte: bool,
    tgt_is_matte: bool,
) -> bool {
    // Compute overlap region in stage coordinates
    let overlap_left = src_rect.0.max(tgt_rect.0);
    let overlap_top = src_rect.1.max(tgt_rect.1);
    let overlap_right = src_rect.2.min(tgt_rect.2);
    let overlap_bottom = src_rect.3.min(tgt_rect.3);

    if overlap_left >= overlap_right || overlap_top >= overlap_bottom {
        return false;
    }

    // Helper: get the matte Arc for a sprite's bitmap
    let get_matte = |sprite_num: i16| -> Option<(Arc<BitmapMask>, u16, u16)> {
        let member_ref = player
            .movie
            .score
            .get_channel(sprite_num)
            .sprite
            .member
            .as_ref()?
            .clone();
        let member = player.movie.cast_manager.find_member_by_ref(&member_ref)?;
        let bmp = member.member_type.as_bitmap()?;
        let bitmap = player.bitmap_manager.get_bitmap(bmp.image_ref)?;
        let matte = bitmap.matte.as_ref()?.clone();
        Some((matte, bitmap.width, bitmap.height))
    };

    // Get matte data for matte-ink sprites
    let src_matte = if src_is_matte {
        get_matte(src_num)
    } else {
        None
    };
    let tgt_matte = if tgt_is_matte {
        get_matte(tgt_num)
    } else {
        None
    };

    // If we need matte data but it's not available (not yet rendered), fall back to AABB
    if (src_is_matte && src_matte.is_none()) || (tgt_is_matte && tgt_matte.is_none()) {
        return true;
    }

    let src_rect_w = (src_rect.2 - src_rect.0).max(1);
    let src_rect_h = (src_rect.3 - src_rect.1).max(1);
    let tgt_rect_w = (tgt_rect.2 - tgt_rect.0).max(1);
    let tgt_rect_h = (tgt_rect.3 - tgt_rect.1).max(1);

    // Check pixel overlap in the overlap region
    for stage_y in overlap_top..overlap_bottom {
        for stage_x in overlap_left..overlap_right {
            // Check source pixel opacity
            let src_opaque = if let Some((ref matte, bw, bh, ..)) = src_matte {
                let bx = ((stage_x - src_rect.0) as u32 * bw as u32 / src_rect_w as u32) as u16;
                let by = ((stage_y - src_rect.1) as u32 * bh as u32 / src_rect_h as u32) as u16;
                matte.get_bit(bx, by)
            } else {
                true // Non-matte sprite: all pixels in bounding box are opaque
            };

            if !src_opaque {
                continue;
            }

            // Check target pixel opacity
            let tgt_opaque = if let Some((ref matte, bw, bh, ..)) = tgt_matte {
                let bx = ((stage_x - tgt_rect.0) as u32 * bw as u32 / tgt_rect_w as u32) as u16;
                let by = ((stage_y - tgt_rect.1) as u32 * bh as u32 / tgt_rect_h as u32) as u16;
                matte.get_bit(bx, by)
            } else {
                true
            };

            if src_opaque && tgt_opaque {
                return true; // Found overlapping opaque pixels
            }
        }
    }

    false // No overlapping opaque pixels
}

pub struct SpriteCompareBytecodeHandler {}

impl SpriteCompareBytecodeHandler {
    /// ontospr - Check if one sprite intersects with another sprite
    /// Pops two values from stack:
    /// - First pop: target sprite (from sprite() call or sprite number)
    /// - Second pop: source sprite number
    /// Pushes 1 if sprites intersect, 0 if they don't
    pub fn onto_sprite(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        let player = &mut *runtime.player;
        let symbols = &mut *runtime.symbols;
        {
            if !ctx.scope.validate_top(player) {
                return Err(crate::player::cancelled_scope_error());
            }
            // Pop the target sprite (result from sprite() call)
            let target_sprite_ref = {
                let (scopes, allocator, bitmap_manager) = (
                    &mut player.scopes,
                    &mut player.allocator,
                    &mut player.bitmap_manager,
                );
                scopes
                    .get_mut(ctx.scope_ref())
                    .unwrap()
                    .stack
                    .pop_ref_with(allocator, bitmap_manager)
                    .unwrap()
            };

            // Pop the source sprite number
            let source_sprite_ref = {
                let (scopes, allocator, bitmap_manager) = (
                    &mut player.scopes,
                    &mut player.allocator,
                    &mut player.bitmap_manager,
                );
                scopes
                    .get_mut(ctx.scope_ref())
                    .unwrap()
                    .stack
                    .pop_ref_with(allocator, bitmap_manager)
                    .unwrap()
            };

            // Get sprite numbers - handle both sprite refs and plain integers
            let get_sprite_num = |datum_ref: &crate::player::DatumRef| -> Result<i16, ScriptError> {
                let datum = checked_sprite_operand(player, symbols, datum_ref)?;

                // Try to_sprite_ref first (proper sprite reference)
                if let Ok(num) = datum.to_sprite_ref() {
                    return Ok(num);
                }

                // Fall back to int_value for plain integers
                if let Ok(num) = datum.int_value() {
                    return Ok(num as i16);
                }

                Err(ScriptError::new(format!(
                    "Expected sprite reference or integer, got {}",
                    diagnostic_datum(datum, symbols, player)
                )))
            };

            let source_sprite_num = get_sprite_num(&source_sprite_ref)?;
            let target_sprite_num = get_sprite_num(&target_sprite_ref)?;

            debug!(
                "ontospr: Comparing sprite {} with sprite {}",
                source_sprite_num, target_sprite_num
            );
            debug!(
                "  source_sprite_ref datum: {}",
                diagnostic_datum(player.get_datum(&source_sprite_ref), symbols, player)
            );
            debug!(
                "  target_sprite_ref datum: {}",
                diagnostic_datum(player.get_datum(&target_sprite_ref), symbols, player)
            );

            // Helper function to get rect bounds
            let mut get_rect_bounds =
                |sprite_num: i16| -> Result<(i32, i32, i32, i32), ScriptError> {
                    let rect_datum = sprite_get_prop(
                        player,
                        symbols,
                        sprite_num,
                        Symbol::builtin(BuiltInSymbol::Rect),
                    )?;

                    debug!(
                        "  sprite {} rect datum: {}",
                        sprite_num,
                        diagnostic_datum(&rect_datum, symbols, player)
                    );

                    // Extract rect coordinates - rect is stored as Datum::Rect([left, top, right, bottom])
                    match rect_datum {
                        Datum::Rect(vals, _flags) => {
                            let left = vals[0] as i32;
                            let top = vals[1] as i32;
                            let right = vals[2] as i32;
                            let bottom = vals[3] as i32;
                            debug!(
                                "  sprite {} rect: [{}, {}, {}, {}]",
                                sprite_num, left, top, right, bottom
                            );
                            Ok((left, top, right, bottom))
                        }
                        Datum::List(_, coords, _) => {
                            // Also support list format [left, top, right, bottom] just in case
                            if coords.len() != 4 {
                                return Err(ScriptError::new(format!(
                                    "Sprite {} rect has invalid format (length {})",
                                    sprite_num,
                                    coords.len()
                                )));
                            }
                            let left = player.get_datum(&coords[0]).int_value()?;
                            let top = player.get_datum(&coords[1]).int_value()?;
                            let right = player.get_datum(&coords[2]).int_value()?;
                            let bottom = player.get_datum(&coords[3]).int_value()?;
                            debug!(
                                "  sprite {} rect: [{}, {}, {}, {}]",
                                sprite_num, left, top, right, bottom
                            );
                            Ok((left, top, right, bottom))
                        }
                        _ => Err(ScriptError::new(format!(
                            "Sprite {} rect is not a rect or list: {}",
                            sprite_num,
                            diagnostic_datum(&rect_datum, symbols, player)
                        ))),
                    }
                };

            // Get rectangles for both sprites
            let source_rect = match get_rect_bounds(source_sprite_num) {
                Ok(rect) => rect,
                Err(e) => {
                    warn!(
                        "WARNING: Failed to get rect for source sprite {}: {:?}",
                        source_sprite_num, e
                    );
                    // Sprite doesn't exist or has no rect, return 0 (no collision)
                    let result_ref = player.alloc_datum(Datum::Int(0));
                    let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                    scope.stack.push(result_ref);
                    return Ok(HandlerExecutionResult::Advance);
                }
            };

            let target_rect = match get_rect_bounds(target_sprite_num) {
                Ok(rect) => rect,
                Err(e) => {
                    warn!(
                        "WARNING: Failed to get rect for target sprite {}: {:?}",
                        target_sprite_num, e
                    );
                    // Sprite doesn't exist or has no rect, return 0 (no collision)
                    let result_ref = player.alloc_datum(Datum::Int(0));
                    let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                    scope.stack.push(result_ref);
                    return Ok(HandlerExecutionResult::Advance);
                }
            };

            // Check if rectangles intersect (AABB test)
            let (src_left, src_top, src_right, src_bottom) = source_rect;
            let (tgt_left, tgt_top, tgt_right, tgt_bottom) = target_rect;

            let mut intersects = !(
                src_right <= tgt_left ||   // source is completely to the left
                src_left >= tgt_right ||   // source is completely to the right
                src_bottom <= tgt_top ||   // source is completely above
                src_top >= tgt_bottom
                // source is completely below
            );

            // Matte ink pixel-level collision check
            // In Director, sprites with matte ink use actual pixel overlap rather
            // than just bounding box intersection.
            if intersects {
                let src_ink = player.movie.score.get_channel(source_sprite_num).sprite.ink;
                let tgt_ink = player.movie.score.get_channel(target_sprite_num).sprite.ink;

                if is_matte_ink(src_ink) || is_matte_ink(tgt_ink) {
                    // Ensure mattes are computed for the matte-ink sprites
                    if is_matte_ink(src_ink) {
                        if let Some((image_ref, _, _)) =
                            get_sprite_image_ref(player, source_sprite_num)
                        {
                            let palettes = player.movie.cast_manager.palettes();
                            if let Some(bmp) = player.bitmap_manager.get_bitmap_mut(image_ref) {
                                if bmp.matte.is_none() {
                                    bmp.create_matte(&palettes);
                                }
                            }
                        }
                    }
                    if is_matte_ink(tgt_ink) {
                        if let Some((image_ref, _, _)) =
                            get_sprite_image_ref(player, target_sprite_num)
                        {
                            let palettes = player.movie.cast_manager.palettes();
                            if let Some(bmp) = player.bitmap_manager.get_bitmap_mut(image_ref) {
                                if bmp.matte.is_none() {
                                    bmp.create_matte(&palettes);
                                }
                            }
                        }
                    }

                    intersects = check_matte_pixel_overlap(
                        player,
                        source_sprite_num,
                        target_sprite_num,
                        source_rect,
                        target_rect,
                        is_matte_ink(src_ink),
                        is_matte_ink(tgt_ink),
                    );
                }
            }

            // Debug logging
            debug!(
                "ontospr: sprite {} [{},{},{},{}] vs sprite {} [{},{},{},{}] => {}",
                source_sprite_num,
                src_left,
                src_top,
                src_right,
                src_bottom,
                target_sprite_num,
                tgt_left,
                tgt_top,
                tgt_right,
                tgt_bottom,
                if intersects {
                    "INTERSECT"
                } else {
                    "no collision"
                }
            );

            // Push result (1 for true, 0 for false)
            let result = if intersects { 1 } else { 0 };
            let result_ref = player.alloc_datum(Datum::Int(result));

            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            scope.stack.push(result_ref);

            Ok(HandlerExecutionResult::Advance)
        }
    }

    /// intospr - Check if one sprite is completely within another sprite
    /// Pops two values from stack:
    /// - First pop: target sprite (the container)
    /// - Second pop: source sprite number (the sprite to check if within)
    /// Pushes 1 if source is completely within target, 0 otherwise
    pub fn into_sprite(
        runtime: &mut crate::player::session::ExecutionContext,
        ctx: &BytecodeHandlerContext,
    ) -> Result<HandlerExecutionResult, ScriptError> {
        let player = &mut *runtime.player;
        let symbols = &mut *runtime.symbols;
        {
            if !ctx.scope.validate_top(player) {
                return Err(crate::player::cancelled_scope_error());
            }
            // Pop the target sprite (the container)
            let target_sprite_ref = {
                let (scopes, allocator, bitmap_manager) = (
                    &mut player.scopes,
                    &mut player.allocator,
                    &mut player.bitmap_manager,
                );
                scopes
                    .get_mut(ctx.scope_ref())
                    .unwrap()
                    .stack
                    .pop_ref_with(allocator, bitmap_manager)
                    .unwrap()
            };

            // Pop the source sprite number (the one to check if within)
            let source_sprite_ref = {
                let (scopes, allocator, bitmap_manager) = (
                    &mut player.scopes,
                    &mut player.allocator,
                    &mut player.bitmap_manager,
                );
                scopes
                    .get_mut(ctx.scope_ref())
                    .unwrap()
                    .stack
                    .pop_ref_with(allocator, bitmap_manager)
                    .unwrap()
            };

            // Get sprite numbers - handle both sprite refs and plain integers
            let get_sprite_num = |datum_ref: &crate::player::DatumRef| -> Result<i16, ScriptError> {
                let datum = checked_sprite_operand(player, symbols, datum_ref)?;

                // Try to_sprite_ref first (proper sprite reference)
                if let Ok(num) = datum.to_sprite_ref() {
                    return Ok(num);
                }

                // Fall back to int_value for plain integers
                if let Ok(num) = datum.int_value() {
                    return Ok(num as i16);
                }

                Err(ScriptError::new(format!(
                    "Expected sprite reference or integer, got {}",
                    diagnostic_datum(datum, symbols, player)
                )))
            };

            let source_sprite_num = get_sprite_num(&source_sprite_ref)?;
            let target_sprite_num = get_sprite_num(&target_sprite_ref)?;

            debug!(
                "intospr: Checking if sprite {} is within sprite {}",
                source_sprite_num, target_sprite_num
            );

            // Helper function to get rect bounds
            let mut get_rect_bounds =
                |sprite_num: i16| -> Result<(i32, i32, i32, i32), ScriptError> {
                    let rect_datum = sprite_get_prop(
                        player,
                        symbols,
                        sprite_num,
                        Symbol::builtin(BuiltInSymbol::Rect),
                    )?;

                    match rect_datum {
                        Datum::Rect(vals, _flags) => {
                            let left = vals[0] as i32;
                            let top = vals[1] as i32;
                            let right = vals[2] as i32;
                            let bottom = vals[3] as i32;
                            Ok((left, top, right, bottom))
                        }
                        Datum::List(_, coords, _) => {
                            if coords.len() != 4 {
                                return Err(ScriptError::new(format!(
                                    "Sprite {} rect has invalid format (length {})",
                                    sprite_num,
                                    coords.len()
                                )));
                            }
                            let left = player.get_datum(&coords[0]).int_value()?;
                            let top = player.get_datum(&coords[1]).int_value()?;
                            let right = player.get_datum(&coords[2]).int_value()?;
                            let bottom = player.get_datum(&coords[3]).int_value()?;
                            Ok((left, top, right, bottom))
                        }
                        _ => Err(ScriptError::new(format!(
                            "Sprite {} rect is not a rect or list: {}",
                            sprite_num,
                            diagnostic_datum(&rect_datum, symbols, player)
                        ))),
                    }
                };

            // Get rectangles for both sprites
            let source_rect = match get_rect_bounds(source_sprite_num) {
                Ok(rect) => rect,
                Err(e) => {
                    warn!(
                        "WARNING: Failed to get rect for source sprite {}: {:?}",
                        source_sprite_num, e
                    );
                    let result_ref = player.alloc_datum(Datum::Int(0));
                    let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                    scope.stack.push(result_ref);
                    return Ok(HandlerExecutionResult::Advance);
                }
            };

            let target_rect = match get_rect_bounds(target_sprite_num) {
                Ok(rect) => rect,
                Err(e) => {
                    warn!(
                        "WARNING: Failed to get rect for target sprite {}: {:?}",
                        target_sprite_num, e
                    );
                    let result_ref = player.alloc_datum(Datum::Int(0));
                    let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
                    scope.stack.push(result_ref);
                    return Ok(HandlerExecutionResult::Advance);
                }
            };

            // Check if source is completely within target
            // Source is within target if all edges of source are inside target
            let (src_left, src_top, src_right, src_bottom) = source_rect;
            let (tgt_left, tgt_top, tgt_right, tgt_bottom) = target_rect;

            let is_within = src_left >= tgt_left
                && src_top >= tgt_top
                && src_right <= tgt_right
                && src_bottom <= tgt_bottom;

            debug!(
                "intospr: sprite {} [{},{},{},{}] within sprite {} [{},{},{},{}] => {}",
                source_sprite_num,
                src_left,
                src_top,
                src_right,
                src_bottom,
                target_sprite_num,
                tgt_left,
                tgt_top,
                tgt_right,
                tgt_bottom,
                if is_within { "WITHIN" } else { "not within" }
            );

            // Push result (1 for true, 0 for false)
            let result = if is_within { 1 } else { 0 };
            let result_ref = player.alloc_datum(Datum::Int(result));

            let scope = player.scopes.get_mut(ctx.scope_ref()).unwrap();
            scope.stack.push(result_ref);

            Ok(HandlerExecutionResult::Advance)
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::{
        cell::RefCell,
        collections::{HashMap, VecDeque},
        rc::Rc,
    };

    use crate::{
        director::{
            chunks::{
                handler::{Bytecode, HandlerDef},
                script::ScriptChunk,
            },
            enums::ScriptType,
            lingo::opcode::OpCode,
        },
        player::{
            bytecode::handler_manager::{BytecodeHandlerContext, HandlerCode},
            cast_lib::CastMemberRef,
            ownership::{OwnerKey, OwnerToken},
            scope::ScopeRef,
            script::Script,
            session::ExecutionContext,
            symbols::{symbol::Symbol, symbol_table::SymbolTable},
            DatumType, DirPlayer, ScopeToken, ScriptErrorCode,
        },
    };

    fn make_player(player_id: u64) -> DirPlayer {
        let (tx, _rx) = async_std::channel::unbounded();
        DirPlayer::new_with_owner(
            tx,
            OwnerToken::new(OwnerKey {
                session: 79,
                player: player_id,
                generation: 1,
            }),
        )
    }

    fn make_context(player: &DirPlayer, slot: ScopeRef, opcode: OpCode) -> BytecodeHandlerContext {
        let handler = Rc::new(HandlerDef {
            name_id: 0,
            bytecode_array: vec![Bytecode::new(opcode, 0, 0)],
            bytecode_index_map: fxhash::FxHashMap::default(),
            argument_name_ids: vec![],
            local_name_ids: vec![],
            global_name_ids: vec![],
            compiled_ir: RefCell::new(None),
        });
        let script = Rc::new(Script {
            member_ref: CastMemberRef {
                cast_lib: 0,
                cast_member: 0,
            },
            name: String::new(),
            chunk: ScriptChunk {
                script_number: 0,
                literals: vec![],
                handlers: vec![],
                property_name_ids: vec![],
                property_defaults: HashMap::new(),
            },
            script_type: ScriptType::Movie,
            handlers: fxhash::FxHashMap::default(),
            handler_names_raw: vec![],
            handler_names: vec![],
            properties: RefCell::new(fxhash::FxHashMap::default()),
        });
        let scope = &player.scopes[slot];
        BytecodeHandlerContext {
            scope: ScopeToken {
                owner: player.owner.clone(),
                slot,
                generation: scope.generation,
                epoch: player.scope_invalidation_epoch,
            },
            code: HandlerCode {
                script,
                handler,
                names: Rc::from(Vec::<Symbol>::new()),
            },
            multiplier: 1,
        }
    }

    fn pop_int(player: &mut DirPlayer, slot: ScopeRef) -> i32 {
        let result_ref = {
            let (scopes, allocator, bitmap_manager) = (
                &mut player.scopes,
                &mut player.allocator,
                &mut player.bitmap_manager,
            );
            scopes[slot]
                .stack
                .pop_ref_with(allocator, bitmap_manager)
                .unwrap()
        };
        player.get_datum(&result_ref).int_value().unwrap()
    }

    fn add_test_channels(player: &mut DirPlayer) {
        player.movie.score.channels = vec![
            crate::player::score::SpriteChannel::new(0),
            crate::player::score::SpriteChannel::new(1),
            crate::player::score::SpriteChannel::new(2),
        ];
    }

    #[test]
    fn onto_and_into_use_real_sprite_channels_with_explicit_context() {
        let mut player = make_player(1);
        add_test_channels(&mut player);
        player.movie.score.channels[1].sprite.loc_h = 5;
        player.movie.score.channels[1].sprite.loc_v = 5;
        player.movie.score.channels[1].sprite.width = 10;
        player.movie.score.channels[1].sprite.height = 10;
        player.movie.score.channels[2].sprite.width = 20;
        player.movie.score.channels[2].sprite.height = 20;
        let slot = player.push_scope();
        let source = player.alloc_datum(Datum::Int(1));
        let target = player.alloc_datum(Datum::Int(2));
        player.scopes[slot].stack.push(source);
        player.scopes[slot].stack.push(target);
        let onto_ctx = make_context(&player, slot, OpCode::OntoSpr);
        let mut symbols = SymbolTable::new();
        {
            let mut runtime = ExecutionContext {
                player_id: 1,
                symbols: &mut symbols,
                player: &mut player,
            };
            SpriteCompareBytecodeHandler::onto_sprite(&mut runtime, &onto_ctx).unwrap();
        }
        assert_eq!(pop_int(&mut player, slot), 1);

        player.movie.score.channels[1].sprite.loc_h = 2;
        player.movie.score.channels[1].sprite.loc_v = 2;
        player.movie.score.channels[1].sprite.width = 4;
        player.movie.score.channels[1].sprite.height = 4;
        let source = player.alloc_datum(Datum::Int(1));
        let target = player.alloc_datum(Datum::Int(2));
        player.scopes[slot].stack.push(source);
        player.scopes[slot].stack.push(target);
        let into_ctx = make_context(&player, slot, OpCode::IntoSpr);
        {
            let mut runtime = ExecutionContext {
                player_id: 1,
                symbols: &mut symbols,
                player: &mut player,
            };
            SpriteCompareBytecodeHandler::into_sprite(&mut runtime, &into_ctx).unwrap();
        }
        assert_eq!(pop_int(&mut player, slot), 1);
        player.pop_scope();
    }

    #[test]
    fn sprite_operands_reject_foreign_symbols_and_outer_refs() {
        let mut player = make_player(1);
        add_test_channels(&mut player);
        player.movie.score.channels[2].sprite.width = 10;
        player.movie.score.channels[2].sprite.height = 10;
        let slot = player.push_scope();
        let mut foreign_symbols = SymbolTable::new();
        let foreign_symbol =
            player.alloc_datum(Datum::Symbol(foreign_symbols.intern("foreignSprite")));
        let target = player.alloc_datum(Datum::Int(2));
        player.scopes[slot].stack.push(foreign_symbol);
        player.scopes[slot].stack.push(target);
        let ctx = make_context(&player, slot, OpCode::OntoSpr);
        let mut symbols = SymbolTable::new();
        let result = {
            let mut runtime = ExecutionContext {
                player_id: 1,
                symbols: &mut symbols,
                player: &mut player,
            };
            SpriteCompareBytecodeHandler::onto_sprite(&mut runtime, &ctx)
        };
        assert!(result.is_err());

        let local_symbol = player.alloc_datum(Datum::Symbol(symbols.intern("localSprite")));
        let target = player.alloc_datum(Datum::Int(2));
        player.scopes[slot].stack.push(local_symbol);
        player.scopes[slot].stack.push(target);
        let ctx = make_context(&player, slot, OpCode::OntoSpr);
        {
            let mut runtime = ExecutionContext {
                player_id: 1,
                symbols: &mut symbols,
                player: &mut player,
            };
            SpriteCompareBytecodeHandler::onto_sprite(&mut runtime, &ctx).unwrap();
        }
        assert_eq!(pop_int(&mut player, slot), 0);

        let mut other = make_player(2);
        let foreign_outer = other.alloc_datum(Datum::Int(1));
        let target = player.alloc_datum(Datum::Int(2));
        player.scopes[slot].stack.push(foreign_outer);
        player.scopes[slot].stack.push(target);
        let ctx = make_context(&player, slot, OpCode::IntoSpr);
        let result = {
            let mut runtime = ExecutionContext {
                player_id: 1,
                symbols: &mut symbols,
                player: &mut player,
            };
            SpriteCompareBytecodeHandler::into_sprite(&mut runtime, &ctx)
        };
        assert!(result.is_err());
        assert_eq!(player.movie.score.channels[1].sprite.loc_h, 0);
        player.pop_scope();
    }

    #[test]
    fn stale_sprite_scope_aborts_before_stack_pop() {
        let mut player = make_player(1);
        add_test_channels(&mut player);
        let slot = player.push_scope();
        let onto_ctx = make_context(&player, slot, OpCode::OntoSpr);
        let into_ctx = make_context(&player, slot, OpCode::IntoSpr);
        player.pop_scope();
        assert_eq!(player.push_scope(), slot);
        player.scopes[slot]
            .stack
            .push_value(crate::player::scope::StackDatum::Int(7));
        let mut symbols = SymbolTable::new();
        let onto_result = {
            let mut runtime = ExecutionContext {
                player_id: 1,
                symbols: &mut symbols,
                player: &mut player,
            };
            SpriteCompareBytecodeHandler::onto_sprite(&mut runtime, &onto_ctx)
        };
        assert_eq!(onto_result.err().unwrap().code, ScriptErrorCode::Abort);
        assert_eq!(player.scopes[slot].stack.len(), 1);

        let into_result = {
            let mut runtime = ExecutionContext {
                player_id: 1,
                symbols: &mut symbols,
                player: &mut player,
            };
            SpriteCompareBytecodeHandler::into_sprite(&mut runtime, &into_ctx)
        };
        assert_eq!(into_result.err().unwrap().code, ScriptErrorCode::Abort);
        assert_eq!(player.scopes[slot].stack.len(), 1);
        player.pop_scope();
    }

    #[test]
    fn diagnostic_fallback_does_not_propagate_formatter_failure() {
        let mut player = make_player(1);
        add_test_channels(&mut player);
        let slot = player.push_scope();
        let mut foreign_symbols = SymbolTable::new();
        let nested_foreign =
            player.alloc_datum(Datum::Symbol(foreign_symbols.intern("nestedForeign")));
        let malformed = player.alloc_datum(Datum::List(
            DatumType::List,
            VecDeque::from([nested_foreign]),
            false,
        ));
        let target = player.alloc_datum(Datum::Int(2));
        player.scopes[slot].stack.push(malformed);
        player.scopes[slot].stack.push(target);
        let ctx = make_context(&player, slot, OpCode::OntoSpr);
        let mut symbols = SymbolTable::new();
        let result = {
            let mut runtime = ExecutionContext {
                player_id: 1,
                symbols: &mut symbols,
                player: &mut player,
            };
            SpriteCompareBytecodeHandler::onto_sprite(&mut runtime, &ctx)
        };
        assert!(result
            .err()
            .unwrap()
            .message
            .contains("<unformattable list>"));
        player.pop_scope();
    }
}
