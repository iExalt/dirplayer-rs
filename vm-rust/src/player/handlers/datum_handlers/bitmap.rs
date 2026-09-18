use std::collections::HashMap;

use fxhash::FxHashMap;

use crate::{
    director::lingo::datum::{datum_bool, Datum},
    player::{
        bitmap::{
            bitmap::{resolve_color_ref, Bitmap, BuiltInPalette, PaletteRef},
            manager::{BitmapHandle, BitmapId},
            mask::BitmapMask,
        },
        geometry::IntRect,
        handlers::types::TypeUtils,
        player_duplicate_datum,
        symbols::{builtin::BuiltInSymbol, symbol::Symbol, symbol_table::SymbolTable},
        ColorRef, DatumRef, DirPlayer, ScriptError, ScriptErrorCode,
    },
};

use super::prop_list::PropListUtils;

pub struct BitmapDatumHandlers {}

fn with_bitmap_context<T, F>(player: &mut DirPlayer, f: F) -> Result<T, ScriptError>
where
    F: FnOnce(&mut DirPlayer) -> Result<T, ScriptError>,
{
    f(player)
}

fn checked_datum<'a>(
    player: &'a DirPlayer,
    datum_ref: &DatumRef,
    symbols: &SymbolTable,
) -> Result<&'a Datum, ScriptError> {
    crate::player::handlers::datum_handlers::cast_member_ref::checked_get_datum(
        player, datum_ref, symbols,
    )
}

fn checked_bitmap_ref(
    player: &DirPlayer,
    datum_ref: &DatumRef,
    symbols: &SymbolTable,
) -> Result<BitmapId, ScriptError> {
    let bitmap_ref = checked_datum(player, datum_ref, symbols)?.to_bitmap_ref()?;
    player.bitmap_manager.local_id(bitmap_ref).ok_or_else(|| {
        ScriptError::new_code(
            ScriptErrorCode::InvalidReference,
            "Invalid bitmap reference".to_string(),
        )
    })
}

impl BitmapDatumHandlers {
    pub fn call(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        handler_name: Symbol,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        // If a mutating op targets the persistent stage framebuffer, mark it
        // dirty so the renderer begins compositing it over the sprite output
        // (the "imaging Lingo" engine pattern — see stage.rs `image` getter).
        let handler_name_text = symbols
            .display(&handler_name)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        if matches!(
            handler_name_text,
            "fill" | "draw" | "setPixel" | "copyPixels" | "applyFilter" | "setAlpha" | "floodFill"
        ) {
            with_bitmap_context(player, |player| {
                if let Datum::BitmapRef(bref) = checked_datum(player, datum, symbols)? {
                    if player.bitmap_manager.local_id(bref) == player.stage_image {
                        player.stage_image_dirty = true;
                    }
                }
                Ok::<(), ScriptError>(())
            })?;
        }
        let handler_name_lower = symbols
            .lower(&handler_name)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        match handler_name_lower {
            "fill" => Self::fill(player, symbols, datum, args),
            "draw" => Self::draw(player, symbols, datum, args),
            "setpixel" => Self::set_pixel(player, symbols, datum, args),
            "extractalpha" => Self::extract_alpha(player, symbols, datum, args),
            "duplicate" => Self::duplicate(player, symbols, datum, args),
            "copypixels" => Self::copy_pixels(player, symbols, datum, args),
            "applyfilter" => Self::apply_filter(player, symbols, datum, args),
            "creatematte" => Self::create_matte(player, symbols, datum, args),
            // NOT an alias for createMatte — see `Bitmap::create_mask`.
            "createmask" => Self::create_mask(player, symbols, datum, args),
            "trimwhitespace" => Self::trim_whitespace(player, symbols, datum, args),
            "getpixel" => Self::get_pixel(player, symbols, datum, args),
            "crop" => Self::crop(player, symbols, datum, args),
            "setalpha" => Self::set_alpha(player, symbols, datum, args),
            "floodfill" => with_bitmap_context(player, |player| {
                // Args: point, color  OR  x, y, color
                if args.len() != 2 && args.len() != 3 {
                    return Err(ScriptError::new(
                        "floodFill requires 2 or 3 arguments".to_string(),
                    ));
                }

                let bitmap_ref = checked_bitmap_ref(player, datum, symbols)?;

                let (x, y, color_arg_idx) = if args.len() == 3 {
                    // floodFill(x, y, color)
                    let x = checked_datum(player, &args[0], symbols)?.int_value()?;
                    let y = checked_datum(player, &args[1], symbols)?.int_value()?;
                    (x, y, 2)
                } else {
                    // floodFill(point, color)
                    let (pt_vals, _flags) =
                        checked_datum(player, &args[0], symbols)?.to_point_inline()?;
                    let x = pt_vals[0] as i32;
                    let y = pt_vals[1] as i32;
                    (x, y, 1)
                };

                let point_tuple = (x, y);

                let color_ref =
                    checked_datum(player, &args[color_arg_idx], symbols)?.to_color_ref()?;

                // Get palettes once
                let palettes = player.movie.cast_manager.palettes();

                // Get bitmap palette and resolve color in one scope
                let target_rgb = {
                    let bitmap = player
                        .bitmap_manager
                        .get_bitmap(bitmap_ref)
                        .ok_or_else(|| ScriptError::new("Invalid bitmap reference".to_string()))?;

                    let palette = bitmap.palette_ref.clone();
                    let rgb = resolve_color_ref(
                        &palettes,
                        &color_ref,
                        &palette,
                        bitmap.original_bit_depth,
                    );
                    rgb
                }; // bitmap borrow ends here

                // Now mutate the bitmap with the resolved color
                let bitmap = player
                    .bitmap_manager
                    .get_bitmap_mut(bitmap_ref)
                    .ok_or_else(|| ScriptError::new("Invalid bitmap reference".to_string()))?;

                bitmap.flood_fill(point_tuple, target_rgb, &palettes);

                Ok(player.alloc_datum(Datum::Void))
            }),
            "getprop" => Self::get_prop_handler(player, symbols, datum, args),
            _ => Err(ScriptError::new(format!(
                "no handler {} for bitmap datum",
                symbols
                    .display(&handler_name)
                    .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
            ))),
        }
    }

    pub fn get_pixel(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        with_bitmap_context(player, |player| {
            let bitmap_ref = checked_bitmap_ref(player, datum, symbols)?;
            let bitmap = player
                .bitmap_manager
                .get_bitmap(bitmap_ref)
                .ok_or_else(|| {
                    ScriptError::new_code(
                        ScriptErrorCode::InvalidReference,
                        "Invalid bitmap reference".to_string(),
                    )
                })?;
            // Parse args: (point [, #integer]) or (x, y)
            let first_is_point =
                matches!(checked_datum(player, &args[0], symbols)?, Datum::Point(..));
            let (x, y, return_integer) = if first_is_point {
                let (pt_vals, _flags) =
                    checked_datum(player, &args[0], symbols)?.to_point_inline()?;
                let x = pt_vals[0] as i32;
                let y = pt_vals[1] as i32;
                let return_integer = if args.len() > 1 {
                    let flag = checked_datum(player, &args[1], symbols)?
                        .string_value(symbols)
                        .unwrap_or_default();
                    flag.eq_ignore_ascii_case("integer")
                } else {
                    false
                };
                (x, y, return_integer)
            } else {
                let x = checked_datum(player, &args[0], symbols)?.int_value()?;
                let y = checked_datum(player, &args[1], symbols)?.int_value()?;
                (x, y, false)
            };
            let color = bitmap.get_pixel_color_ref(x as u16, y as u16);
            if return_integer {
                let palettes = player.movie.cast_manager.palettes();
                let (r, g, b) = crate::player::bitmap::bitmap::resolve_color_ref(
                    &palettes,
                    &color,
                    &bitmap.palette_ref,
                    bitmap.original_bit_depth,
                );
                // Director's getPixel(pt, #integer) returns the pixel's value in
                // the bitmap's native format, not always 24-bit RGB:
                //   - 8-bit: palette index (0..255)
                //   - 16-bit: RGB555 packed word (0..32767)
                //   - 32-bit: 24-bit RGB (0..16_777_215)
                // Many classic Lingo hit-test handlers compare getPixel against
                // 16-bit color constants (e.g. 32767 for white / transparent marker);
                // returning 24-bit RGB here broke pixel-accurate avatar click
                // tests so clicks on transparent pixels registered as hits.
                let int_color = match bitmap.original_bit_depth {
                    1 | 2 | 4 | 8 => {
                        if let crate::player::sprite::ColorRef::PaletteIndex(idx) = color {
                            idx as i32
                        } else {
                            ((r as i32) << 16) | ((g as i32) << 8) | (b as i32)
                        }
                    }
                    16 => {
                        // Pack as RGB555 (Director's 16-bit format). The original
                        // file may have had the high bit set (giving 65535 vs 32767
                        // for white) but that bit is lost during decode; return the
                        // 15-bit value which matches the common 32767 transparent
                        // marker. Scripts that accept either 32767 or 65535 (which
                        // is the standard hit-test idiom) work correctly.
                        let r5 = (r as i32 >> 3) & 0x1F;
                        let g5 = (g as i32 >> 3) & 0x1F;
                        let b5 = (b as i32 >> 3) & 0x1F;
                        (r5 << 10) | (g5 << 5) | b5
                    }
                    _ => ((r as i32) << 16) | ((g as i32) << 8) | (b as i32),
                };
                Ok(player.alloc_datum(Datum::Int(int_color)))
            } else {
                let color_ref = player.alloc_datum(Datum::ColorRef(color));
                Ok(color_ref)
            }
        })
    }

    pub fn trim_whitespace(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        _: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        with_bitmap_context(player, |player| {
            let bitmap = checked_bitmap_ref(player, datum, symbols)?;
            let bitmap = player
                .bitmap_manager
                .get_bitmap_mut(bitmap)
                .ok_or_else(|| {
                    ScriptError::new_code(
                        ScriptErrorCode::InvalidReference,
                        "Invalid bitmap reference".to_string(),
                    )
                })?;
            bitmap.trim_whitespace(&player.movie.cast_manager.palettes());
            Ok(datum.clone())
        })
    }

    pub fn create_matte(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        with_bitmap_context(player, |player| {
            // Director: imageObject.createMatte({alphaThreshold}). The
            // alphaThreshold (0..255) excludes pixels whose alpha falls
            // below that value from the resulting matte; only meaningful
            // for 32-bit images with an alpha channel. We don't honour
            // the threshold yet (our matte builder uses the bg color key
            // path, see Bitmap::create_matte), but accept and ignore the
            // argument so scripts that pass it don't error out.
            if args.len() > 1 {
                return Err(ScriptError::new(
                    "createMatte takes at most 1 argument (alphaThreshold)".to_string(),
                ));
            }
            let bitmap = checked_bitmap_ref(player, datum, symbols)?;
            let bitmap = player
                .bitmap_manager
                .get_bitmap_mut(bitmap)
                .ok_or_else(|| {
                    ScriptError::new_code(
                        ScriptErrorCode::InvalidReference,
                        "Invalid bitmap reference".to_string(),
                    )
                })?;
            bitmap.create_matte(&player.movie.cast_manager.palettes());
            let matte_arc = bitmap.matte.as_ref().unwrap().clone();
            Ok(player.alloc_datum(Datum::Matte(matte_arc)))
        })
    }

    /// Director: `imageObject.createMask()`. Takes no arguments (unlike
    /// createMatte's optional alphaThreshold) per the 11.5 dictionary.
    pub fn create_mask(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        with_bitmap_context(player, |player| {
            if !args.is_empty() {
                return Err(ScriptError::new(
                    "createMask takes no arguments".to_string(),
                ));
            }
            let bitmap_ref = checked_bitmap_ref(player, datum, symbols)?;
            let palettes = player.movie.cast_manager.palettes();
            let bitmap = player
                .bitmap_manager
                .get_bitmap(bitmap_ref)
                .ok_or_else(|| ScriptError::new("createMask: bitmap not found".to_string()))?;
            let mask = bitmap.create_mask(&palettes);
            Ok(player.alloc_datum(Datum::Matte(std::sync::Arc::new(mask))))
        })
    }

    pub fn duplicate(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        _: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        player_duplicate_datum(player, symbols, datum)
    }

    pub fn crop(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        with_bitmap_context(player, |player| {
            if args.len() != 1 {
                return Err(ScriptError::new(
                    "crop requires 1 argument (rect)".to_string(),
                ));
            }

            let bitmap_ref = checked_bitmap_ref(player, datum, symbols)?;
            let (rect_vals, _flags) = checked_datum(player, &args[0], symbols)?.to_rect_inline()?;

            let left = rect_vals[0] as i32;
            let top = rect_vals[1] as i32;
            let right = rect_vals[2] as i32;
            let bottom = rect_vals[3] as i32;

            // Calculate cropped dimensions
            let crop_width = (right - left).max(0) as u16;
            let crop_height = (bottom - top).max(0) as u16;

            if crop_width == 0 || crop_height == 0 {
                return Err(ScriptError::new(
                    "crop rect must have positive dimensions".to_string(),
                ));
            }

            let src_bitmap = player
                .bitmap_manager
                .get_bitmap(bitmap_ref)
                .ok_or_else(|| ScriptError::new("Invalid bitmap reference".to_string()))?;

            // Create new bitmap with cropped dimensions, preserving bit depth and palette
            let mut cropped_bitmap = crate::player::bitmap::bitmap::Bitmap::new(
                crop_width,
                crop_height,
                src_bitmap.bit_depth,
                src_bitmap.original_bit_depth,
                if src_bitmap.use_alpha { 8 } else { 0 },
                src_bitmap.palette_ref.clone(),
            );
            cropped_bitmap.use_alpha = src_bitmap.use_alpha;
            cropped_bitmap.trim_white_space = src_bitmap.trim_white_space;
            // Same depth: copy the rows straight, so the alpha channel survives.
            // Anything else still goes through the ink path.
            if src_bitmap.bit_depth == 32 && cropped_bitmap.bit_depth == 32 {
                cropped_bitmap = src_bitmap.crop_rgba(left, top, crop_width, crop_height);
            } else {
                let palettes = player.movie.cast_manager.palettes();
                let src_rect = IntRect::from(left, top, right, bottom);
                let dst_rect = IntRect::from(0, 0, crop_width as i32, crop_height as i32);
                let params = crate::player::bitmap::drawing::CopyPixelsParams::default(&src_bitmap);
                let src_bitmap_clone = src_bitmap.clone();
                cropped_bitmap.copy_pixels_with_params(
                    &palettes,
                    &src_bitmap_clone,
                    dst_rect,
                    src_rect,
                    &params,
                );
            }

            // Ephemeral: `bitmap.duplicate(rect)` produces a fresh bitmap not
            // owned by any cast member. Free when the wrapping DatumRef drops.
            let new_bitmap_ref = player.bitmap_manager.add_ephemeral_bitmap(cropped_bitmap);
            let handle = player.bitmap_handle_for_id(new_bitmap_ref)?;
            Ok(player.alloc_datum(Datum::BitmapRef(handle)))
        })
    }

    pub fn extract_alpha(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        _args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        with_bitmap_context(player, |player| {
            let bitmap_ref = checked_bitmap_ref(player, datum, symbols)?;
            let src = player
                .bitmap_manager
                .get_bitmap(bitmap_ref)
                .ok_or_else(|| ScriptError::new("Invalid bitmap reference".to_string()))?;

            let w = src.width;
            let h = src.height;
            let is_32bit = src.bit_depth == 32;

            // Create an 8-bit grayscale bitmap for the alpha channel
            let mut alpha_bitmap = crate::player::bitmap::bitmap::Bitmap::new(
                w,
                h,
                32,
                32,
                0,
                src.palette_ref.clone(),
            );

            if is_32bit {
                // Extract alpha from 32-bit RGBA data (4 bytes per pixel, alpha = byte 3)
                let row_bytes = w as usize * 4;
                for y in 0..h as usize {
                    for x in 0..w as usize {
                        let src_idx = y * row_bytes + x * 4;
                        let alpha = if src_idx + 3 < src.data.len() {
                            src.data[src_idx + 3]
                        } else {
                            255
                        };
                        // Write grayscale: R=G=B=alpha, A=255
                        let dst_idx = y * row_bytes + x * 4;
                        if dst_idx + 3 < alpha_bitmap.data.len() {
                            alpha_bitmap.data[dst_idx] = alpha;
                            alpha_bitmap.data[dst_idx + 1] = alpha;
                            alpha_bitmap.data[dst_idx + 2] = alpha;
                            alpha_bitmap.data[dst_idx + 3] = 255;
                        }
                    }
                }
            } else {
                // Non-32-bit: no alpha channel, return all-white (fully opaque)
                alpha_bitmap.data.fill(255);
            }

            // Ephemeral: `bitmap.extractAlpha()` returns a fresh derived bitmap
            // not owned by a cast member. Free when the wrapping DatumRef drops.
            let new_ref = player.bitmap_manager.add_ephemeral_bitmap(alpha_bitmap);
            let handle = player.bitmap_handle_for_id(new_ref)?;
            Ok(player.alloc_datum(Datum::BitmapRef(handle)))
        })
    }

    pub fn set_alpha(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        with_bitmap_context(player, |player| {
            if args.len() != 1 {
                return Err(ScriptError::new("setAlpha requires 1 argument".to_string()));
            }

            let bitmap_ref = checked_bitmap_ref(player, datum, symbols)?;
            let arg = checked_datum(player, &args[0], symbols)?;

            // Check if target bitmap is 32-bit
            let (width, height, bit_depth) = {
                let bitmap = player
                    .bitmap_manager
                    .get_bitmap(bitmap_ref)
                    .ok_or_else(|| ScriptError::new("Invalid bitmap reference".to_string()))?;
                (bitmap.width, bitmap.height, bitmap.bit_depth)
            };

            if bit_depth != 32 {
                // setAlpha only works on 32-bit images
                log::warn!("setAlpha called on non-32-bit bitmap");
                return Ok(player.alloc_datum(datum_bool(false)));
            }

            match arg {
                Datum::Int(alpha_level) => {
                    // Set all pixels to a flat alpha level (0-255)
                    let alpha = (*alpha_level).clamp(0, 255) as u8;
                    let bitmap = player
                        .bitmap_manager
                        .get_bitmap_mut(bitmap_ref)
                        .ok_or_else(|| ScriptError::new("Invalid bitmap reference".to_string()))?;

                    // For 32-bit images, data is RGBA, so we modify every 4th byte (alpha channel)
                    for i in (3..bitmap.data.len()).step_by(4) {
                        bitmap.data[i] = alpha;
                    }
                    bitmap.use_alpha = true;
                    // bitmap.version += 1;

                    Ok(player.alloc_datum(datum_bool(true)))
                }
                Datum::BitmapRef(alpha_bitmap_ref) => {
                    // Set alpha from an 8-bit grayscale image
                    let alpha_bitmap = player
                        .bitmap_manager
                        .get_bitmap_handle(alpha_bitmap_ref)
                        .ok_or_else(|| {
                            ScriptError::new("Invalid alpha bitmap reference".to_string())
                        })?;

                    // Alpha image must be 8-bit
                    if alpha_bitmap.bit_depth != 8 {
                        return Ok(player.alloc_datum(datum_bool(false)));
                    }

                    // Both images must have the same dimensions
                    if alpha_bitmap.width != width || alpha_bitmap.height != height {
                        return Ok(player.alloc_datum(datum_bool(false)));
                    }

                    // Clone the alpha data to avoid borrow issues
                    let alpha_data = alpha_bitmap.data.clone();

                    let bitmap = player
                        .bitmap_manager
                        .get_bitmap_mut(bitmap_ref)
                        .ok_or_else(|| ScriptError::new("Invalid bitmap reference".to_string()))?;

                    // Copy alpha values from the 8-bit image to the alpha channel of the 32-bit image.
                    // Director's setAlpha uses raw palette indices as alpha values directly:
                    // index 0 = transparent, index 255 = opaque.
                    for y in 0..height {
                        for x in 0..width {
                            let alpha_idx = y as usize * width as usize + x as usize;
                            let dst_idx = (y as usize * width as usize + x as usize) * 4 + 3;
                            if alpha_idx < alpha_data.len() && dst_idx < bitmap.data.len() {
                                bitmap.data[dst_idx] = alpha_data[alpha_idx];
                            }
                        }
                    }
                    bitmap.use_alpha = true;

                    Ok(player.alloc_datum(datum_bool(true)))
                }
                _ => {
                    // Invalid argument type
                    Ok(player.alloc_datum(datum_bool(false)))
                }
            }
        })
    }

    pub fn draw(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        with_bitmap_context(player, |player| {
            let bitmap_ref = checked_bitmap_ref(player, datum, symbols)?;
            if args.is_empty() {
                return Err(ScriptError::new("draw requires arguments".to_string()));
            }
            let first_arg = checked_datum(player, &args[0], symbols)?;
            let mut arg_pos = 1;
            let (x1, y1, x2, y2) = match first_arg {
                Datum::Int(x1) => {
                    if args.len() < 4 {
                        return Err(ScriptError::new(
                            "draw(x1, y1, x2, y2, ...) requires at least 4 arguments".to_string(),
                        ));
                    }
                    let y1 = checked_datum(player, &args[arg_pos], symbols)?.int_value()?;
                    arg_pos += 1;
                    let x2 = checked_datum(player, &args[arg_pos], symbols)?.int_value()?;
                    arg_pos += 1;
                    let y2 = checked_datum(player, &args[arg_pos], symbols)?.int_value()?;
                    arg_pos += 1;
                    (*x1, y1, x2, y2)
                }
                Datum::Point(point_vals, _flags) => {
                    let x1 = point_vals[0] as i32;
                    let y1 = point_vals[1] as i32;
                    let (point2_vals, _flags) =
                        checked_datum(player, &args[arg_pos], symbols)?.to_point_inline()?;
                    arg_pos += 1;
                    let x2 = point2_vals[0] as i32;
                    let y2 = point2_vals[1] as i32;
                    (x1, y1, x2, y2)
                }
                Datum::Rect(rect_vals, _flags) => {
                    let x1 = rect_vals[0] as i32;
                    let y1 = rect_vals[1] as i32;
                    let x2 = rect_vals[2] as i32;
                    let y2 = rect_vals[3] as i32;
                    (x1, y1, x2, y2)
                }
                _ => {
                    return Err(ScriptError::new(
                        "First argument to draw must be coordinates, a point, or a rect"
                            .to_string(),
                    ))
                }
            };

            // Handle optional color argument before the prop list
            // draw(x1, y1, x2, y2, [color,] propList)
            let mut explicit_color = if arg_pos + 1 < args.len() {
                let maybe_color = checked_datum(player, &args[arg_pos], symbols)?;
                if matches!(maybe_color, Datum::ColorRef(_)) {
                    let c = maybe_color.to_color_ref().ok();
                    arg_pos += 1;
                    c
                } else {
                    None
                }
            } else {
                None
            };

            // The final `colorObjOrParamList` argument is EITHER a plain color
            // object OR a parameter list ([#shapeType, #lineSize, #color, ...]).
            // (11.5 Scripting Dictionary: draw(rect, colorObjOrParamList) — "A
            // color object or parameter list"; the default #shapeType is #line.)
            // When it is a bare color there is no list, so draw a 1-pixel #line
            // in that color and let every prop lookup below fall through to its
            // default via an empty map. Tetris' make_table draws grid lines with
            // `the_image.draw(rect, rgb(255,255,255))`, which previously errored
            // ("Cannot convert datum to map") trying to to_map() the color.
            let empty_map: std::collections::VecDeque<(DatumRef, DatumRef)> =
                std::collections::VecDeque::new();
            let (draw_map, draw_map_sorted): (
                &std::collections::VecDeque<(DatumRef, DatumRef)>,
                bool,
            ) = if arg_pos >= args.len() {
                (&empty_map, false)
            } else {
                let last_arg = checked_datum(player, &args[arg_pos], symbols)?;
                if matches!(last_arg, Datum::ColorRef(_)) {
                    if explicit_color.is_none() {
                        explicit_color = last_arg.to_color_ref().ok();
                    }
                    (&empty_map, false)
                } else {
                    last_arg.to_map_tuple()?
                }
            };
            let bitmap = player
                .bitmap_manager
                .get_bitmap(bitmap_ref)
                .ok_or_else(|| {
                    ScriptError::new_code(
                        ScriptErrorCode::InvalidReference,
                        "Invalid bitmap reference".to_string(),
                    )
                })?;

            let color_ref = if let Some(c) = explicit_color {
                c
            } else {
                let cr = PropListUtils::get_by_concrete_key(
                    &draw_map,
                    &Datum::Symbol(Symbol::builtin(BuiltInSymbol::Color)),
                    &player.allocator,
                    symbols,
                    draw_map_sorted,
                )?;
                checked_datum(player, &cr, symbols)?.to_color_ref()?
            };
            let palettes = player.movie.cast_manager.palettes();
            let color = resolve_color_ref(
                &palettes,
                &color_ref,
                &bitmap.palette_ref,
                bitmap.original_bit_depth,
            );

            // Director 11.5 Scripting Dictionary, draw() image method: the
            // `#shapeType` property "is a symbol value of #oval, #rect,
            // #roundRect, or #line. The default is #line." When the param list
            // omits #shapeType, fall back to #line rather than treating the
            // VOID lookup result as a literal "VOID" shape name.
            let shape_type_d = PropListUtils::get_by_concrete_key(
                &draw_map,
                &Datum::Symbol(Symbol::builtin(BuiltInSymbol::ShapeType)),
                &player.allocator,
                symbols,
                draw_map_sorted,
            )?;
            let shape_type_d = checked_datum(player, &shape_type_d, symbols)?;
            let shape_type = if shape_type_d.is_void() {
                "line".to_string()
            } else {
                shape_type_d.string_value(symbols)?
            };

            let blend = PropListUtils::get_by_concrete_key(
                &draw_map,
                &Datum::Symbol(Symbol::builtin(BuiltInSymbol::Blend)),
                &player.allocator,
                symbols,
                draw_map_sorted,
            )?;
            let blend = checked_datum(player, &blend, symbols)?;
            let blend = if blend.is_void() {
                100
            } else {
                blend.int_value()?
            };

            // Director chapter 15: optional `#lineSize` (default 1) controls
            // stroke thickness for rect/oval/roundRect/line outlines. Some
            // scripts use alias keys (#lineWidth, #width, #strokeWidth) for
            // the same property — accept all four.
            let mut thickness = 1;
            for key in [
                BuiltInSymbol::LineSize,
                BuiltInSymbol::LineWidth,
                BuiltInSymbol::Width,
                BuiltInSymbol::StrokeWidth,
            ] {
                let value = match PropListUtils::get_by_concrete_key(
                    &draw_map,
                    &Datum::Symbol(Symbol::builtin(key)),
                    &player.allocator,
                    symbols,
                    draw_map_sorted,
                ) {
                    Ok(value) => value,
                    Err(error) if error.code == ScriptErrorCode::InvalidReference => {
                        return Err(error)
                    }
                    Err(_) => continue,
                };
                let value = checked_datum(player, &value, symbols)?;
                if !value.is_void() {
                    if let Ok(number) = value.int_value() {
                        thickness = number.max(1);
                        break;
                    }
                }
            }

            // Optional `#radius` for #roundRect — defaults to 8 (Director's
            // visual default for the rounded-rect tool).
            let radius_d = PropListUtils::get_by_concrete_key(
                &draw_map,
                &Datum::Symbol(Symbol::builtin(BuiltInSymbol::Radius)),
                &player.allocator,
                symbols,
                draw_map_sorted,
            )?;
            let radius_d = checked_datum(player, &radius_d, symbols)?;
            let radius = if radius_d.is_void() {
                8
            } else {
                radius_d.int_value()?.max(0)
            };

            let bitmap = player
                .bitmap_manager
                .get_bitmap_mut(bitmap_ref)
                .ok_or_else(|| {
                    ScriptError::new_code(
                        ScriptErrorCode::InvalidReference,
                        "Invalid bitmap reference".to_string(),
                    )
                })?;
            let alpha = blend as f32 / 100.0;
            // `#roundRect` arrives as its display spelling, so compare
            // case-insensitively as Director does.
            match_ci!(shape_type, {
                "rect" => {
                    bitmap.stroke_rect(x1, y1, x2, y2, color, &palettes, alpha);
                },
                "oval" => {
                    bitmap.stroke_ellipse(x1, y1, x2, y2, color, &palettes, alpha, thickness);
                },
                "roundRect" => {
                    bitmap.stroke_round_rect(x1, y1, x2, y2, radius, color, &palettes, alpha, thickness);
                },
                "line" => {
                    // For #line, (x1,y1) and (x2,y2) are the line endpoints
                    // (rather than a bounding rect like the other shapes).
                    bitmap.draw_line_thick(x1, y1, x2, y2, color, &palettes, alpha, thickness);
                },
                _ => {
                    return Err(ScriptError::new(format!(
                        "Invalid shapeType '#{}' for draw (expected #rect, #oval, #roundRect, #line)",
                        shape_type
                    )));
                }
            });
            Ok(datum.clone())
        })
    }

    pub fn set_pixel(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        with_bitmap_context(player, |player| {
            let bitmap_ref = checked_bitmap_ref(player, datum, symbols)?;

            // setPixel supports both (x, y, color) and (point, color) forms
            let (x, y, color_obj_or_int, bit_depth, original_bit_depth, palette_ref) = {
                let bitmap = player
                    .bitmap_manager
                    .get_bitmap(bitmap_ref)
                    .ok_or_else(|| {
                        ScriptError::new_code(
                            ScriptErrorCode::InvalidReference,
                            "Invalid bitmap reference".to_string(),
                        )
                    })?;

                let first_arg = checked_datum(player, &args[0], symbols)?;
                let (x, y, color_obj_or_int) = if let Datum::Point(pt_vals, _flags) = first_arg {
                    let px = pt_vals[0] as i32;
                    let py = pt_vals[1] as i32;
                    let color = checked_datum(player, &args[1], symbols)?;
                    (px, py, color)
                } else {
                    let x = first_arg.int_value()?;
                    let y = checked_datum(player, &args[1], symbols)?.int_value()?;
                    let color = checked_datum(player, &args[2], symbols)?;
                    (x, y, color)
                };

                if x < 0 || y < 0 || x >= bitmap.width as i32 || y >= bitmap.height as i32 {
                    return Ok(player.alloc_datum(datum_bool(false)));
                }

                (
                    x,
                    y,
                    color_obj_or_int.to_owned(),
                    bitmap.bit_depth,
                    bitmap.original_bit_depth,
                    bitmap.palette_ref.clone(),
                )
            };

            let palettes = player.movie.cast_manager.palettes();
            let bitmap = player
                .bitmap_manager
                .get_bitmap_mut(bitmap_ref)
                .ok_or_else(|| {
                    ScriptError::new_code(
                        ScriptErrorCode::InvalidReference,
                        "Invalid bitmap reference".to_string(),
                    )
                })?;

            if color_obj_or_int.is_int() {
                let int_value = color_obj_or_int.int_value()?;
                if bit_depth == 8 {
                    // 8-bit: treat as palette index → grayscale
                    let idx = int_value as u8;
                    bitmap.set_pixel(x, y, (idx, idx, idx), &palettes);
                } else {
                    // 16/32-bit: treat as packed RGB integer (r*65536 + g*256 + b)
                    let r = ((int_value >> 16) & 0xFF) as u8;
                    let g = ((int_value >> 8) & 0xFF) as u8;
                    let b = (int_value & 0xFF) as u8;
                    bitmap.set_pixel(x, y, (r, g, b), &palettes);
                }
            } else {
                let color = color_obj_or_int.to_color_ref()?;
                let color = resolve_color_ref(&palettes, &color, &palette_ref, original_bit_depth);
                bitmap.set_pixel(x, y, color, &palettes);
            }

            Ok(player.alloc_datum(datum_bool(true)))
        })
    }

    pub fn fill(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        with_bitmap_context(player, |player| {
            // Check the receiver before looking at the argument list, as the
            // original handler did, while postponing BitmapId conversion
            // until after the argument forms have been parsed.
            let receiver = checked_datum(player, datum, symbols)?;
            if args.is_empty() {
                return Err(ScriptError::new("fill requires arguments".to_string()));
            }

            // Parse the region, mirroring draw(): coordinates (left,top,right,
            // bottom), two points, or a rect. (11.5 Scripting Dictionary:
            // fill(left,top,right,bottom, ...) / fill(point,point, ...) /
            // fill(rect, ...).)
            let first = checked_datum(player, &args[0], symbols)?;
            let mut arg_pos = 1;
            let (x1, y1, x2, y2) = match first {
                Datum::Int(x1) => {
                    if args.len() < 5 {
                        return Err(ScriptError::new(
                            "fill(left, top, right, bottom, ...) requires at least 5 arguments"
                                .to_string(),
                        ));
                    }
                    let y1 = checked_datum(player, &args[1], symbols)?.int_value()?;
                    let x2 = checked_datum(player, &args[2], symbols)?.int_value()?;
                    let y2 = checked_datum(player, &args[3], symbols)?.int_value()?;
                    arg_pos = 4;
                    (*x1, y1, x2, y2)
                }
                Datum::Point(p1, _flags) => {
                    let x1 = p1[0] as i32;
                    let y1 = p1[1] as i32;
                    let (p2, _flags) =
                        checked_datum(player, &args[1], symbols)?.to_point_inline()?;
                    arg_pos = 2;
                    (x1, y1, p2[0] as i32, p2[1] as i32)
                }
                Datum::Rect(r, _flags) => (r[0] as i32, r[1] as i32, r[2] as i32, r[3] as i32),
                _ => {
                    return Err(ScriptError::new(
                        "First argument to fill must be coordinates, a point, or a rect"
                            .to_string(),
                    ))
                }
            };

            // The final `colorObjOrParamList` argument is EITHER a plain color
            // OR a parameter list. Director also accepts an explicit color
            // BEFORE the list (`fill(region, color, [#shapeType: ...])`, as
            // Tetris' name-input table does) — peel it off first when something
            // follows it.
            let mut explicit_color: Option<ColorRef> = None;
            if arg_pos + 1 < args.len() {
                let maybe_color = checked_datum(player, &args[arg_pos], symbols)?;
                match maybe_color {
                    Datum::ColorRef(_) => {
                        explicit_color = maybe_color.to_color_ref().ok().cloned();
                        arg_pos += 1;
                    }
                    Datum::Int(i) => {
                        explicit_color = Some(ColorRef::PaletteIndex(*i as u8));
                        arg_pos += 1;
                    }
                    _ => {}
                }
            }

            if arg_pos >= args.len() {
                return Err(ScriptError::new(
                    "Invalid number of arguments for fill".to_string(),
                ));
            }

            let params = checked_datum(player, &args[arg_pos], symbols)?;
            let (color_ref, shape) = match params {
                Datum::ColorRef(color_ref) => {
                    (color_ref.clone(), Symbol::builtin(BuiltInSymbol::Rect))
                }
                Datum::Int(i) => (
                    ColorRef::PaletteIndex(*i as u8),
                    Symbol::builtin(BuiltInSymbol::Rect),
                ),
                Datum::PropList(prop_list, prop_list_sorted) => {
                    let shape_ref = PropListUtils::get_by_concrete_key(
                        &prop_list,
                        &Datum::Symbol(symbols.intern("shapeType")),
                        &player.allocator,
                        symbols,
                        *prop_list_sorted,
                    )?;
                    let shape = match checked_datum(player, &shape_ref, symbols)? {
                        Datum::Symbol(s) => {
                            symbols.display(s).map_err(|_| {
                                crate::player::symbols::symbol::SymbolError::Foreign
                            })?;
                            s.clone()
                        }
                        Datum::Void => Symbol::builtin(BuiltInSymbol::Rect),
                        _ => {
                            return Err(ScriptError::new(
                                "Invalid shapeType in fill prop list".to_string(),
                            ))
                        }
                    };
                    // Color comes from the explicit leading arg if present,
                    // otherwise from the list's #color entry.
                    let color_ref = if let Some(c) = explicit_color.clone() {
                        c
                    } else {
                        let cr = PropListUtils::get_by_concrete_key(
                            &prop_list,
                            &Datum::Symbol(Symbol::builtin(BuiltInSymbol::Color)),
                            &player.allocator,
                            symbols,
                            *prop_list_sorted,
                        )?;
                        checked_datum(player, &cr, symbols)?.to_color_ref()?.clone()
                    };
                    (color_ref, shape)
                }
                _ => return Err(ScriptError::new("Invalid parameter for fill".to_string())),
            };
            let rect_i32 = (x1, y1, x2, y2);
            let bitmap_ref = match receiver {
                Datum::BitmapRef(bitmap_ref) => {
                    let Some(bitmap_id) = player.bitmap_manager.local_id(bitmap_ref) else {
                        return Err(ScriptError::new_code(
                            ScriptErrorCode::InvalidReference,
                            "Invalid bitmap reference".to_string(),
                        ));
                    };
                    bitmap_id
                }
                _ => return Err(ScriptError::new("Cannot fill non-bitmap".to_string())),
            };
            let (x1, y1, x2, y2) = rect_i32;
            let bitmap = player
                .bitmap_manager
                .get_bitmap(bitmap_ref)
                .ok_or_else(|| {
                    ScriptError::new_code(
                        ScriptErrorCode::InvalidReference,
                        "Invalid bitmap reference".to_string(),
                    )
                })?;
            let palettes = player.movie.cast_manager.palettes();
            let color = resolve_color_ref(
                &palettes,
                &color_ref,
                &bitmap.palette_ref,
                bitmap.original_bit_depth,
            );
            let bitmap = player
                .bitmap_manager
                .get_bitmap_mut(bitmap_ref)
                .ok_or_else(|| {
                    ScriptError::new_code(
                        ScriptErrorCode::InvalidReference,
                        "Invalid bitmap reference".to_string(),
                    )
                })?;
            // `image.fill(rect, [#color:.., #shapeType:..])` supports filled
            // rect / oval / roundRect (Director 11.5 Scripting Dictionary).
            // The worldMap reveals completed levels by punching white #oval
            // holes into a grey overlay (`wmGreyBuffer.fill(dR, [#color:
            // rgb(255,255,255), #shapeType: #oval])`) then keying them out
            // with ink 36 — so without oval support the whole map stays grey.
            match symbols
                .lower(&shape)
                .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
            {
                "oval" => bitmap.fill_ellipse(x1, y1, x2, y2, color, &palettes, 1.0),
                "roundrect" => bitmap.fill_round_rect(x1, y1, x2, y2, 12, color, &palettes, 1.0),
                _ => bitmap.fill_rect(x1, y1, x2, y2, color, &palettes, 1.0),
            }
            Ok(datum.clone())
        })
    }

    pub fn copy_pixels(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        with_bitmap_context(player, |player| {
            let dst_bitmap_ref = checked_bitmap_ref(player, datum, symbols)?;
            let src_bitmap_ref = checked_datum(player, &args[0], symbols)?;
            let src_bitmap_ref = if src_bitmap_ref.is_void()
                || (src_bitmap_ref.is_number() && src_bitmap_ref.int_value()? == 0)
            {
                return Ok(datum.clone());
            } else {
                let bitmap_ref = src_bitmap_ref.to_bitmap_ref()?;
                let bitmap_id = player.bitmap_manager.local_id(bitmap_ref).ok_or_else(|| {
                    ScriptError::new_code(
                        ScriptErrorCode::InvalidReference,
                        "Invalid bitmap reference".to_string(),
                    )
                })?;
                if player.bitmap_manager.get_bitmap(bitmap_id).is_none() {
                    return Err(ScriptError::new_code(
                        ScriptErrorCode::InvalidReference,
                        "Invalid bitmap reference".to_string(),
                    ));
                }
                bitmap_id
            };
            let dest_rect_or_quad = checked_datum(player, &args[1], symbols)?;
            let (src_rect_vals, _flags) =
                checked_datum(player, &args[2], symbols)?.to_rect_inline()?;
            let sx1 = src_rect_vals[0] as i32;
            let sy1 = src_rect_vals[1] as i32;
            let sx2 = src_rect_vals[2] as i32;
            let sy2 = src_rect_vals[3] as i32;
            let param_list = args
                .get(3)
                .map(|x| checked_datum(player, x, symbols))
                .transpose()?;
            let mut param_list_concrete = HashMap::new();
            if let Some(param_list) = param_list {
                if let Datum::PropList(param_list, ..) = param_list {
                    for (key, value) in param_list {
                        let key = checked_datum(player, key, symbols)?.string_value(symbols)?;
                        let value = checked_datum(player, value, symbols)?.clone();
                        param_list_concrete.insert(key, value);
                    }
                }
            }

            // Pre-convert BitmapId maskImage to BitmapMask
            // Director's #maskImage param accepts a bitmap where white=transparent, black=opaque
            if let Some(Datum::BitmapRef(mask_ref)) = param_list_concrete.get("maskImage") {
                let mask_bitmap = player
                    .bitmap_manager
                    .get_bitmap_handle(mask_ref)
                    .ok_or_else(|| {
                        ScriptError::new_code(
                            ScriptErrorCode::InvalidReference,
                            "Invalid mask bitmap reference".to_string(),
                        )
                    })?;
                let palettes = player.movie.cast_manager.palettes();
                let w = mask_bitmap.width;
                let h = mask_bitmap.height;
                let mut mask = BitmapMask::new(w, h, false);
                for y in 0..h {
                    for x in 0..w {
                        let (r, g, b) = mask_bitmap.get_pixel_color(&palettes, x, y);
                        let luminance = (r as u16 + g as u16 + b as u16) / 3;
                        if luminance <= 128 {
                            mask.set_bit(x, y, true);
                        }
                    }
                }
                param_list_concrete.insert(
                    "maskImage".to_string(),
                    Datum::Matte(std::sync::Arc::new(mask)),
                );
            }

            // Decode dest as either Rect (axis-aligned blit) or List of 4 Points (quad warp).
            enum DestShape {
                Rect(IntRect),
                Quad([(i32, i32); 4]),
            }
            let dest_shape = match dest_rect_or_quad {
                Datum::Rect(rect_vals, _flags) => {
                    let x1 = rect_vals[0] as i32;
                    let y1 = rect_vals[1] as i32;
                    let x2 = rect_vals[2] as i32;
                    let y2 = rect_vals[3] as i32;
                    DestShape::Rect(IntRect::from_tuple((x1, y1, x2, y2)))
                }
                Datum::List(_, list_val, _) => {
                    let p1 = {
                        let (pv, _f) =
                            checked_datum(player, &list_val[0], symbols)?.to_point_inline()?;
                        (pv[0] as i32, pv[1] as i32)
                    };
                    let p2 = {
                        let (pv, _f) =
                            checked_datum(player, &list_val[1], symbols)?.to_point_inline()?;
                        (pv[0] as i32, pv[1] as i32)
                    };
                    let p3 = {
                        let (pv, _f) =
                            checked_datum(player, &list_val[2], symbols)?.to_point_inline()?;
                        (pv[0] as i32, pv[1] as i32)
                    };
                    let p4 = {
                        let (pv, _f) =
                            checked_datum(player, &list_val[3], symbols)?.to_point_inline()?;
                        (pv[0] as i32, pv[1] as i32)
                    };
                    // Detect axis-aligned quad (top.y==top.y, etc.) — those
                    // map cleanly to a Rect and let the existing fast path
                    // run with ink / blend / matte support. Otherwise route
                    // through the inverse-bilinear quad warp, which
                    // currently supports copy ink only.
                    let axis_aligned = p1.1 == p2.1 && p4.1 == p3.1 && p1.0 == p4.0 && p2.0 == p3.0;
                    if axis_aligned {
                        DestShape::Rect(IntRect::from_quad(p1, p2, p3, p4))
                    } else {
                        DestShape::Quad([p1, p2, p3, p4])
                    }
                }
                _ => {
                    return Err(ScriptError::new(
                        "Invalid destRect for copyPixels".to_string(),
                    ))
                }
            };
            let src_bitmap = player
                .bitmap_manager
                .get_bitmap(src_bitmap_ref)
                .ok_or_else(|| {
                    ScriptError::new_code(
                        ScriptErrorCode::InvalidReference,
                        "Invalid bitmap reference".to_string(),
                    )
                })?
                .clone();
            let palettes = player.movie.cast_manager.palettes();
            let dst_bitmap = player
                .bitmap_manager
                .get_bitmap_mut(dst_bitmap_ref)
                .ok_or_else(|| {
                    ScriptError::new_code(
                        ScriptErrorCode::InvalidReference,
                        "Invalid bitmap reference".to_string(),
                    )
                })?;

            match dest_shape {
                DestShape::Rect(dest_rect) => {
                    dst_bitmap.copy_pixels_with_symbols(
                        &palettes,
                        &src_bitmap,
                        dest_rect,
                        IntRect::from_tuple((sx1, sy1, sx2, sy2)),
                        &param_list_concrete,
                        Some(&player.movie.score),
                        symbols,
                    )?;
                }
                DestShape::Quad(quad) => {
                    dst_bitmap.copy_pixels_quad(
                        &palettes,
                        &src_bitmap,
                        quad,
                        IntRect::from_tuple((sx1, sy1, sx2, sy2)),
                        &param_list_concrete,
                    );
                }
            }
            Ok(datum.clone())
        })
    }

    /// Director chapter 15 `image.applyFilter(filterObj)`. Mutates the bitmap
    /// in place. The filter is the PropList produced by the global `filter()`
    /// constructor — its `#filterType` symbol decides the dispatch.
    ///
    /// Currently implemented:
    ///   - `#adjustcolorfilter` — applies hue / saturation / contrast /
    ///     brightness using Adobe Flash's AdjustColor convention (see source
    ///     reference in `apply_adjust_color_filter`).
    ///
    /// Other filter symbols (#blurfilter / #glowfilter / etc.) are accepted
    /// without crashing but produce a warning and leave the bitmap unchanged
    /// — this matches the AGEIA Xtra's behaviour for unimplemented filters.
    pub fn apply_filter(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        with_bitmap_context(player, |player| {
            if args.is_empty() {
                return Err(ScriptError::new(
                    "applyFilter requires a filter argument".to_string(),
                ));
            }
            let bitmap_ref = checked_bitmap_ref(player, datum, symbols)?;

            // Read the filter PropList. Lookup is case-insensitive on symbol /
            // string keys to match Director's convention.
            let (filter_type, props, filter_color) = match checked_datum(player, &args[0], symbols)?
            {
                Datum::PropList(items, _) => {
                    let mut filter_type: Option<Symbol> = None;
                    let mut props: FxHashMap<Symbol, f64> = FxHashMap::default();
                    // #color is a colour, not a number — glow/dropShadow need it.
                    let mut filter_color: Option<(u8, u8, u8)> = None;
                    for (k, v) in items.iter() {
                        let key = match checked_datum(player, k, symbols)? {
                            Datum::Symbol(s) => s.clone(),
                            Datum::String(s) => symbols.intern(s),
                            _ => continue,
                        };
                        if key.into_builtin() == Some(BuiltInSymbol::FilterType) {
                            filter_type = checked_datum(player, v, symbols)?
                                .symbol_value(symbols)
                                .ok();
                        } else if key.into_builtin() == Some(BuiltInSymbol::Color) {
                            if let Datum::ColorRef(cr) = checked_datum(player, v, symbols)? {
                                let palettes = player.movie.cast_manager.palettes();
                                filter_color = Some(crate::player::bitmap::bitmap::resolve_color_ref(
                                    &palettes, cr,
                                    &crate::player::bitmap::bitmap::PaletteRef::BuiltIn(
                                        crate::player::bitmap::bitmap::get_system_default_palette(),
                                    ),
                                    32,
                                ));
                            }
                        } else {
                            // Numeric properties for AdjustColor.
                            let val = checked_datum(player, v, symbols)?
                                .float_value()
                                .unwrap_or(0.0);
                            props.insert(key, val);
                        }
                    }
                    (filter_type, props, filter_color)
                }
                _ => {
                    return Err(ScriptError::new(
                        "applyFilter argument is not a filter object".to_string(),
                    ));
                }
            };

            let kind = filter_type.unwrap_or_default();
            match kind.into_builtin() {
                Some(BuiltInSymbol::AdjustColorFilter) => {
                    let bitmap = player
                        .bitmap_manager
                        .get_bitmap_mut(bitmap_ref)
                        .ok_or_else(|| {
                            ScriptError::new("applyFilter: invalid bitmap".to_string())
                        })?;
                    let brightness = props
                        .get(&Symbol::builtin(BuiltInSymbol::Brightness))
                        .copied()
                        .unwrap_or(0.0)
                        .clamp(-100.0, 100.0);
                    let contrast = props
                        .get(&Symbol::builtin(BuiltInSymbol::Contrast))
                        .copied()
                        .unwrap_or(0.0)
                        .clamp(-100.0, 100.0);
                    let saturation = props
                        .get(&Symbol::builtin(BuiltInSymbol::Saturation))
                        .copied()
                        .unwrap_or(0.0)
                        .clamp(-100.0, 100.0);
                    let hue = props
                        .get(&Symbol::builtin(BuiltInSymbol::Hue))
                        .copied()
                        .unwrap_or(0.0)
                        .clamp(-180.0, 180.0);
                    apply_adjust_color_filter(bitmap, brightness, contrast, saturation, hue);
                    bitmap.mark_dirty();
                }
                // Outer glow / drop shadow. Both composite a blurred, coloured
                // copy of the source's ALPHA *behind* the original; a drop shadow
                // is just a glow offset by (distance, angle). Director/Flash
                // measure `angle` clockwise from +x with y pointing DOWN, so the
                // default 45 puts the shadow down-right.
                //
                // AreaZero's whole UI depends on these: every text style carries
                // `#filters: [<colour>OuterGlow, <colour>DropShadow]`, and without
                // them the baked strings have no dark edge and wash out against the
                // bright 3D scene behind the menu.
                Some(BuiltInSymbol::GlowFilter) | Some(BuiltInSymbol::DropShadowFilter) => {
                    let bitmap = player
                        .bitmap_manager
                        .get_bitmap_mut(bitmap_ref)
                        .ok_or_else(|| {
                            ScriptError::new("applyFilter: invalid bitmap".to_string())
                        })?;
                    let blur_x = props
                        .get(&symbols.intern("blurx"))
                        .copied()
                        .unwrap_or(4.0)
                        .max(0.0);
                    let blur_y = props
                        .get(&symbols.intern("blury"))
                        .copied()
                        .unwrap_or(4.0)
                        .max(0.0);
                    // Director expresses strength as a percentage.
                    let strength = props
                        .get(&symbols.intern("strengthpercent"))
                        .copied()
                        .or_else(|| props.get(&symbols.intern("strength")).map(|v| v * 100.0))
                        .unwrap_or(100.0)
                        / 100.0;
                    let quality = props
                        .get(&Symbol::builtin(BuiltInSymbol::Quality))
                        .copied()
                        .unwrap_or(1.0)
                        .clamp(1.0, 3.0) as u32;
                    let (mut off_x, mut off_y) = (0.0f64, 0.0f64);
                    if kind.into_builtin() == Some(BuiltInSymbol::DropShadowFilter) {
                        let distance = props
                            .get(&Symbol::builtin(BuiltInSymbol::Distance))
                            .copied()
                            .unwrap_or(4.0);
                        let angle = props.get(&symbols.intern("angle")).copied().unwrap_or(45.0);
                        let rad = angle.to_radians();
                        off_x = distance * rad.cos();
                        off_y = distance * rad.sin();
                    }
                    let color = filter_color.unwrap_or((0, 0, 0));
                    apply_glow_shadow_filter(
                        bitmap,
                        color,
                        blur_x,
                        blur_y,
                        quality,
                        strength,
                        off_x.round() as i32,
                        off_y.round() as i32,
                    );
                    bitmap.mark_dirty();
                }
                Some(BuiltInSymbol::EmptyString) => {
                    return Err(ScriptError::new(
                        "applyFilter: filter object has no #filterType".to_string(),
                    ));
                }
                other => {
                    log::warn!(
                        "applyFilter: filter type '#{}' is not implemented \u{2014} bitmap unchanged",
                        other.map(|x| symbols.display(&Symbol::builtin(x))).transpose()
                            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
                            .unwrap_or_default()
                    );
                }
            }

            Ok(datum.clone())
        })
    }

    pub fn get_prop_handler(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        if args.len() == 0 {
            return Err(ScriptError::new(
                "getProp requires at least 1 argument".to_string(),
            ));
        }
        with_bitmap_context(player, |player| {
            let prop = crate::player::handlers::datum_handlers::cast_member_ref::checked_get_datum(
                player, &args[0], symbols,
            )?
            .symbol_value(symbols)?;
            let prop_value = Self::get_prop(player, symbols, datum, prop)?;
            if args.len() == 1 {
                Ok(prop_value)
            } else if args.len() == 2 {
                let prop_key_ref = args[1].clone();
                TypeUtils::get_sub_prop(&prop_value, &prop_key_ref, player, symbols)
            } else {
                Err(ScriptError::new(
                    "getProp with sub-property requires 2 arguments".to_string(),
                ))
            }
        })
    }

    pub fn get_prop(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        prop: Symbol,
    ) -> Result<DatumRef, ScriptError> {
        symbols
            .display(&prop)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        let bitmap_ref = checked_bitmap_ref(player, datum, symbols)?;
        let bitmap = player
            .bitmap_manager
            .get_bitmap(bitmap_ref)
            .ok_or_else(|| {
                ScriptError::new_code(
                    ScriptErrorCode::InvalidReference,
                    "Invalid bitmap reference".to_string(),
                )
            })?;
        let width = bitmap.width as i32;
        let height = bitmap.height as i32;
        let result = match prop.into_builtin() {
            Some(BuiltInSymbol::Width) => Ok(Datum::Int(width)),
            Some(BuiltInSymbol::Height) => Ok(Datum::Int(height)),
            Some(BuiltInSymbol::Rect) => {
                Ok(Datum::Rect([0.0, 0.0, width as f64, height as f64], 0))
            }
            Some(BuiltInSymbol::Depth) => Ok(Datum::Int(bitmap.bit_depth as i32)),
            Some(BuiltInSymbol::PaletteRef) => {
                if let PaletteRef::BuiltIn(palette) = bitmap.palette_ref {
                    Ok(Datum::Symbol(Symbol::builtin(palette.symbol())))
                } else {
                    Ok(Datum::PaletteRef(bitmap.palette_ref.to_owned()))
                }
            }
            Some(BuiltInSymbol::Ilk) => Ok(Datum::Symbol(Symbol::builtin(BuiltInSymbol::Image))),
            Some(BuiltInSymbol::UseAlpha) => Ok(Datum::Int(if bitmap.use_alpha { 1 } else { 0 })),
            _ => Err(ScriptError::new(format!(
                "Cannot get bitmap property {}",
                symbols
                    .display(&prop)
                    .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
            ))),
        }?;
        Ok(player.alloc_datum(result))
    }

    pub fn set_bitmap_ref_prop(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        bitmap_ref: &BitmapHandle,
        prop: Symbol,
        value: &DatumRef,
    ) -> Result<(), ScriptError> {
        symbols
            .display(&prop)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        if player
            .bitmap_manager
            .get_bitmap_handle(bitmap_ref)
            .is_none()
        {
            return Err(ScriptError::new_code(
                ScriptErrorCode::InvalidReference,
                "Invalid bitmap reference".to_string(),
            ));
        }
        let value = checked_datum(player, value, symbols)?;
        match prop.into_builtin() {
            Some(BuiltInSymbol::PaletteRef) => match value {
                Datum::Symbol(symbol) => {
                    let palette = BuiltInPalette::from_symbol(symbol.clone(), symbols)?
                        .ok_or_else(|| {
                            ScriptError::new("Invalid built-in palette symbol".to_string())
                        })?;
                    let bitmap = player
                        .bitmap_manager
                        .get_bitmap_handle_mut(bitmap_ref)
                        .ok_or_else(|| {
                            ScriptError::new_code(
                                ScriptErrorCode::InvalidReference,
                                "Invalid bitmap reference".to_string(),
                            )
                        })?;
                    bitmap.palette_ref = PaletteRef::BuiltIn(palette);
                    Ok(())
                }
                Datum::CastMember(member_ref) => {
                    let member_ref = member_ref.to_owned();
                    let bitmap = player
                        .bitmap_manager
                        .get_bitmap_handle_mut(bitmap_ref)
                        .ok_or_else(|| {
                            ScriptError::new_code(
                                ScriptErrorCode::InvalidReference,
                                "Invalid bitmap reference".to_string(),
                            )
                        })?;
                    bitmap.palette_ref = PaletteRef::Member(member_ref);
                    Ok(())
                }
                Datum::PaletteRef(palette_ref) => {
                    let palette_ref = palette_ref.to_owned();
                    let bitmap = player
                        .bitmap_manager
                        .get_bitmap_handle_mut(bitmap_ref)
                        .ok_or_else(|| {
                            ScriptError::new_code(
                                ScriptErrorCode::InvalidReference,
                                "Invalid bitmap reference".to_string(),
                            )
                        })?;
                    bitmap.palette_ref = palette_ref;
                    Ok(())
                }
                _ => Err(ScriptError::new(format!(
                    "Cannot set paletteRef to datum of type {}",
                    value.type_str()
                ))),
            },
            Some(BuiltInSymbol::UseAlpha) => {
                let use_alpha = value.to_bool()?;
                let bitmap = player
                    .bitmap_manager
                    .get_bitmap_handle_mut(bitmap_ref)
                    .ok_or_else(|| {
                        ScriptError::new_code(
                            ScriptErrorCode::InvalidReference,
                            "Invalid bitmap reference".to_string(),
                        )
                    })?;
                bitmap.use_alpha = use_alpha;
                Ok(())
            }
            _ => Err(ScriptError::new(format!(
                "Cannot set bitmap property {}",
                symbols
                    .display(&prop)
                    .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
            ))),
        }
    }
}

/// Adobe Flash AdjustColor convention (Director chapter 15 inherits this from
/// the Flash Bitmap Filters surface):
///   brightness  ∈ [-100, 100]   — additive luminance shift; ±100 maps to
///                                  ±100/255 added per channel in 0..1 space.
///   contrast    ∈ [-100, 100]   — multiplicative around 0.5 grey;
///                                  factor = (100 + contrast) / 100.
///   saturation  ∈ [-100, 100]   — chroma scale; -100 = full grayscale,
///                                  0 = identity, +100 = doubled chroma.
///                                  factor = (100 + saturation) / 100.
///   hue         ∈ [-180, 180]   — hue rotation in degrees around the
///                                  Rec. 601 luma axis.
///
/// Order matches Flash's internal pipeline: hue → saturation → contrast →
/// brightness. (Some implementations swap the last two; Flash applies
/// contrast then brightness, which is what we do here.)
///
/// Operates in-place on RGB channels; alpha is preserved. Supports 32-bit and
/// 16-bit bitmaps (the common cases for textures); 8-bit / palette bitmaps
/// would need a roundtrip through the palette and are out of scope for now —
/// the function emits a debug-log warning and skips paletted images.
/// Composite a blurred, coloured copy of `bitmap`'s alpha BEHIND the image.
///
/// This is the shared core of Director's `#glowFilter` (no offset) and
/// `#dropShadowFilter` (offset by distance/angle). Flash-family semantics:
/// the source's ALPHA is blurred, multiplied by `strength`, tinted with
/// `color`, and the original is drawn over the top.
///
/// The blur is a separable box blur repeated `quality` times — three passes of
/// a box blur is the standard cheap approximation of a Gaussian, and is what
/// the Flash filters themselves do.
///
/// Only meaningful for 32-bit images with an alpha channel; a paletted source
/// has no alpha to blur, so it is left alone.
fn apply_glow_shadow_filter(
    bitmap: &mut Bitmap,
    color: (u8, u8, u8),
    blur_x: f64,
    blur_y: f64,
    quality: u32,
    strength: f64,
    offset_x: i32,
    offset_y: i32,
) {
    if bitmap.bit_depth != 32 {
        log::warn!(
            "applyFilter(#glow/#dropShadow): bitmap is {}-bit; needs alpha, skipped",
            bitmap.bit_depth
        );
        return;
    }
    let w = bitmap.width as usize;
    let h = bitmap.height as usize;
    if w == 0 || h == 0 {
        return;
    }

    // Source alpha, normalised.
    let mut a: Vec<f32> = (0..w * h)
        .map(|i| bitmap.data[i * 4 + 3] as f32 / 255.0)
        .collect();

    // Separable box blur. Radius is half the Flash blur amount (blurX is the
    // full extent of the kernel, not its radius).
    let rx = (blur_x / 2.0).round().max(0.0) as usize;
    let ry = (blur_y / 2.0).round().max(0.0) as usize;
    let mut tmp = vec![0.0f32; w * h];
    for _ in 0..quality.max(1) {
        if rx > 0 {
            for y in 0..h {
                for x in 0..w {
                    let lo = x.saturating_sub(rx);
                    let hi = (x + rx).min(w - 1);
                    let mut sum = 0.0;
                    for s in lo..=hi {
                        sum += a[y * w + s];
                    }
                    tmp[y * w + x] = sum / ((hi - lo + 1) as f32);
                }
            }
            a.copy_from_slice(&tmp);
        }
        if ry > 0 {
            for x in 0..w {
                for y in 0..h {
                    let lo = y.saturating_sub(ry);
                    let hi = (y + ry).min(h - 1);
                    let mut sum = 0.0;
                    for s in lo..=hi {
                        sum += a[s * w + x];
                    }
                    tmp[y * w + x] = sum / ((hi - lo + 1) as f32);
                }
            }
            a.copy_from_slice(&tmp);
        }
    }

    // Composite the tinted, offset blur under the original (dest-over).
    let (sr, sg, sb) = (color.0 as f32, color.1 as f32, color.2 as f32);
    let src = bitmap.data.clone();
    for y in 0..h {
        for x in 0..w {
            let di = (y * w + x) * 4;
            // Sample the blur at the shadow offset.
            let sx = x as i32 - offset_x;
            let sy = y as i32 - offset_y;
            let shadow_a = if sx >= 0 && sy >= 0 && (sx as usize) < w && (sy as usize) < h {
                (a[sy as usize * w + sx as usize] * strength as f32).clamp(0.0, 1.0)
            } else {
                0.0
            };
            if shadow_a <= 0.0 {
                continue;
            }
            let fa = src[di + 3] as f32 / 255.0;
            let out_a = fa + shadow_a * (1.0 - fa);
            if out_a <= 0.0 {
                continue;
            }
            for (c, sc) in [(0usize, sr), (1, sg), (2, sb)] {
                let fc = src[di + c] as f32;
                let out_c = (fc * fa + sc * shadow_a * (1.0 - fa)) / out_a;
                bitmap.data[di + c] = out_c.round().clamp(0.0, 255.0) as u8;
            }
            bitmap.data[di + 3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    bitmap.use_alpha = true;
}

fn apply_adjust_color_filter(
    bitmap: &mut Bitmap,
    brightness: f64,
    contrast: f64,
    saturation: f64,
    hue_deg: f64,
) {
    if bitmap.bit_depth < 16 {
        log::warn!(
            "applyFilter(#adjustColorFilter): bitmap is {}-bit / paletted; skipped",
            bitmap.bit_depth
        );
        return;
    }

    // Pre-compute the contributions of each step so we can apply them per pixel.
    let cos_h = (hue_deg.to_radians()).cos();
    let sin_h = (hue_deg.to_radians()).sin();
    // Rec. 601 luma weights — Flash uses these for the AdjustColor filter.
    let lr = 0.213_f64;
    let lg = 0.715_f64;
    let lb = 0.072_f64;
    // Hue-rotation matrix (RGB → RGB) around the luminance axis. Standard
    // formulation: M = L + cos·(I − L) + sin·R, where L is the projection
    // onto the luma axis (constant rows) and R is the rotation matrix in the
    // chroma plane.
    let m00 = lr + cos_h * (1.0 - lr) + sin_h * (-lr);
    let m01 = lg + cos_h * (-lg) + sin_h * (-lg);
    let m02 = lb + cos_h * (-lb) + sin_h * (1.0 - lb);
    let m10 = lr + cos_h * (-lr) + sin_h * (0.143);
    let m11 = lg + cos_h * (1.0 - lg) + sin_h * (0.140);
    let m12 = lb + cos_h * (-lb) + sin_h * (-0.283);
    let m20 = lr + cos_h * (-lr) + sin_h * (-(1.0 - lr));
    let m21 = lg + cos_h * (-lg) + sin_h * (lg);
    let m22 = lb + cos_h * (1.0 - lb) + sin_h * (lb);

    let sat_factor = (100.0 + saturation) / 100.0;
    let contrast_factor = (100.0 + contrast) / 100.0;
    let bright_shift = brightness / 100.0; // 0..1 scale (±100 maps to ±1.0).

    let width = bitmap.width as i32;
    let height = bitmap.height as i32;
    let bytes_per_pixel = (bitmap.bit_depth / 8) as usize;

    // Snapshot the existing matte (if any) so we can restore it after
    // set_pixel — set_pixel clears it because it doesn't know we're not
    // changing the alpha shape, just the RGB values.
    let saved_matte = bitmap.matte.clone();

    // We need a palette map for resolve_color_ref / set_pixel. AdjustColor on
    // 16/32-bit images doesn't actually need the palette but the API requires
    // one — pass an empty map (resolve_color_ref handles None palettes).
    let palettes = crate::player::bitmap::palette_map::PaletteMap::new();

    for y in 0..height {
        for x in 0..width {
            // Read alpha first (preserve through transform).
            let (r0, g0, b0, a) = if bitmap.bit_depth == 32 {
                let idx = (y as usize * width as usize + x as usize) * bytes_per_pixel;
                (
                    bitmap.data[idx],
                    bitmap.data[idx + 1],
                    bitmap.data[idx + 2],
                    bitmap.data[idx + 3],
                )
            } else {
                // 16-bit: get_pixel_color_with_alpha resolves through palette.
                bitmap.get_pixel_color_with_alpha(&palettes, x as u16, y as u16)
            };

            // 0..1 floats.
            let mut r = r0 as f64 / 255.0;
            let mut g = g0 as f64 / 255.0;
            let mut b = b0 as f64 / 255.0;

            // 1) Hue rotation.
            let hr = m00 * r + m01 * g + m02 * b;
            let hg = m10 * r + m11 * g + m12 * b;
            let hb = m20 * r + m21 * g + m22 * b;
            r = hr;
            g = hg;
            b = hb;

            // 2) Saturation: blend between luminance grayscale and original.
            let lum = lr * r + lg * g + lb * b;
            r = lum + (r - lum) * sat_factor;
            g = lum + (g - lum) * sat_factor;
            b = lum + (b - lum) * sat_factor;

            // 3) Contrast: scale around 0.5 grey.
            r = (r - 0.5) * contrast_factor + 0.5;
            g = (g - 0.5) * contrast_factor + 0.5;
            b = (b - 0.5) * contrast_factor + 0.5;

            // 4) Brightness: additive shift.
            r += bright_shift;
            g += bright_shift;
            b += bright_shift;

            // Clamp to 0..1 and back to u8.
            let r_u = (r.clamp(0.0, 1.0) * 255.0).round() as u8;
            let g_u = (g.clamp(0.0, 1.0) * 255.0).round() as u8;
            let b_u = (b.clamp(0.0, 1.0) * 255.0).round() as u8;

            // Write RGB. For 32-bit we write directly to preserve the existing
            // alpha byte (set_pixel doesn't take alpha). For 16-bit we use
            // set_pixel which roundtrips through the 5550 packing.
            if bitmap.bit_depth == 32 {
                let idx = (y as usize * width as usize + x as usize) * bytes_per_pixel;
                bitmap.data[idx] = r_u;
                bitmap.data[idx + 1] = g_u;
                bitmap.data[idx + 2] = b_u;
                // Alpha untouched (a is the original).
                let _ = a;
            } else {
                bitmap.set_pixel(x, y, (r_u, g_u, b_u), &palettes);
            }
        }
    }

    bitmap.matte = saved_matte;
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::player::bitmap::bitmap::PaletteRef;
    use crate::player::ownership::OwnerToken;
    use async_std::channel;

    fn test_player() -> DirPlayer {
        let (tx, _rx) = channel::unbounded();
        DirPlayer::new_with_owner(tx, OwnerToken::transitional())
    }

    #[test]
    fn direct_bitmap_handlers_produce_duplicate_copy_and_reject_stale_or_foreign_handles() {
        let mut player = test_player();
        let mut symbols = SymbolTable::new();
        let source_handle = player
            .bitmap_manager
            .add_ephemeral_bitmap_handle(Bitmap::new(1, 1, 32, 32, 8, PaletteRef::Default))
            .unwrap();
        player
            .bitmap_manager
            .get_bitmap_handle_mut(&source_handle)
            .unwrap()
            .data
            .copy_from_slice(&[10, 20, 30, 255]);
        let source = player.alloc_datum(Datum::BitmapRef(source_handle.clone()));

        let x = player.alloc_datum(Datum::Int(0));
        let y = player.alloc_datum(Datum::Int(0));
        let pixel = BitmapDatumHandlers::get_pixel(&mut player, &mut symbols, &source, &vec![x, y])
            .expect("valid bitmap getPixel should produce a color");
        assert!(matches!(player.get_datum(&pixel), Datum::ColorRef(_)));

        let duplicate = BitmapDatumHandlers::duplicate(&mut player, &mut symbols, &source, &vec![])
            .expect("valid bitmap duplicate should produce a bitmap");
        assert!(matches!(player.get_datum(&duplicate), Datum::BitmapRef(_)));

        let destination_handle = player
            .bitmap_manager
            .add_ephemeral_bitmap_handle(Bitmap::new(1, 1, 32, 32, 8, PaletteRef::Default))
            .unwrap();
        let destination = player.alloc_datum(Datum::BitmapRef(destination_handle.clone()));
        let destination_rect = player.alloc_datum(Datum::Rect([0.0, 0.0, 1.0, 1.0], 0));
        let source_rect = player.alloc_datum(Datum::Rect([0.0, 0.0, 1.0, 1.0], 0));
        BitmapDatumHandlers::copy_pixels(
            &mut player,
            &mut symbols,
            &destination,
            &vec![source.clone(), destination_rect, source_rect],
        )
        .expect("valid bitmap copyPixels should succeed");
        assert_eq!(
            player
                .bitmap_manager
                .get_bitmap_handle(&destination_handle)
                .unwrap()
                .data,
            vec![10, 20, 30, 255]
        );

        let use_alpha = player.alloc_datum(Datum::Int(1));
        BitmapDatumHandlers::set_bitmap_ref_prop(
            &mut player,
            &mut symbols,
            &destination_handle,
            Symbol::builtin(BuiltInSymbol::UseAlpha),
            &use_alpha,
        )
        .expect("valid bitmap property setter should succeed");
        assert!(
            player
                .bitmap_manager
                .get_bitmap_handle(&destination_handle)
                .unwrap()
                .use_alpha
        );

        let stale = source_handle.clone();
        player.bitmap_manager.rotate_handles().unwrap();
        let stale_x = player.alloc_datum(Datum::Int(0));
        let stale_y = player.alloc_datum(Datum::Int(0));
        assert!(BitmapDatumHandlers::get_pixel(
            &mut player,
            &mut symbols,
            &source,
            &vec![stale_x, stale_y],
        )
        .is_err());
        assert!(BitmapDatumHandlers::set_bitmap_ref_prop(
            &mut player,
            &mut symbols,
            &stale,
            Symbol::builtin(BuiltInSymbol::UseAlpha),
            &use_alpha,
        )
        .is_err());

        let mut foreign_player = test_player();
        let foreign = foreign_player
            .bitmap_manager
            .add_bitmap_handle(Bitmap::new(1, 1, 32, 32, 8, PaletteRef::Default))
            .unwrap();
        assert!(BitmapDatumHandlers::set_bitmap_ref_prop(
            &mut player,
            &mut symbols,
            &foreign,
            Symbol::builtin(BuiltInSymbol::UseAlpha),
            &use_alpha,
        )
        .is_err());
    }
}
