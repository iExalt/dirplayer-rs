use log::debug;

use crate::{
    director::lingo::datum::Datum,
    player::{
        datum_formatting::format_concrete_datum,
        compare::validate_direct_symbol_fields,
        session::ExecutionContext,
        DatumRef, ScriptError,
    },
};

pub struct StringHandlers {}

impl StringHandlers {
    pub fn space(runtime: &mut ExecutionContext<'_>, _: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        Ok(runtime.player.alloc_datum(Datum::String(" ".to_string())))
    }

    pub fn offset(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let str_to_find = checked_datum(player, symbols, &args[0])?.string_value(symbols)?;
            let find_in = checked_datum(player, symbols, &args[1])?.string_value(symbols)?;

            // Lingo edge cases
            if str_to_find.is_empty() {
                return Ok(player.alloc_datum(Datum::Int(1)));
            }

            if find_in.is_empty() {
                return Ok(player.alloc_datum(Datum::Int(0)));
            }

            // Case-insensitive search (like Mac Lingo)
            let find_in_lower = find_in.to_lowercase();
            let str_to_find_lower = str_to_find.to_lowercase();

            let result = find_in_lower
                .find(&str_to_find_lower)
                .map(|byte_index| {
                    // Count characters up to the found byte index
                    let char_index = find_in[..byte_index].chars().count() as i32;
                    char_index + 1 // 1-based indexing
                })
                .unwrap_or(0); // Not found → return 0

            Ok(player.alloc_datum(Datum::Int(result)))
        })
    }

    pub fn length(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let obj = checked_datum(player, symbols, &args[0])?;
            match obj {
                Datum::String(s) => Ok(player.alloc_datum(Datum::Int(s.chars().count() as i32))),
                Datum::StringChunk(..) => {
                    let s = obj.string_value(symbols)?;
                    Ok(player.alloc_datum(Datum::Int(s.chars().count() as i32)))
                }
                // Director coerces VOID to EMPTY in string contexts, so
                // `length(VOID)` is `length("")` = 0. This underpins the very
                // common "is this set?" idiom `if length(me.prop) > 0` on an
                // uninitialised property (which reads VOID) — e.g. Neopets DGS
                // `setFlashLoaderVars`: `if length(me.gameVersion) > 0`.
                Datum::Void => Ok(player.alloc_datum(Datum::Int(0))),
                _ => Err(ScriptError::new(
                    "Cannot get length of non-string".to_string(),
                )),
            }
        })
    }

    pub fn string(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let obj = checked_datum(player, symbols, &args[0])?;
            let result_obj = if obj.is_string() {
                Datum::String(obj.string_value(symbols)?)
            } else if obj.is_void() {
                Datum::String("".to_string())
            } else if let Datum::Symbol(s) = obj {
                // In Director, string(#symbol) returns "symbol" without the # prefix
                Datum::String(symbols.display(s).map_err(|_| ScriptError::new("foreign symbol".to_owned()))?.to_owned())
            } else {
                Datum::String(format_concrete_datum(obj, symbols, player)?)
            };
            Ok(player.alloc_datum(result_obj))
        })
    }

    pub fn chars(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let string = checked_datum(player, symbols, &args[0])?
                .string_value(symbols)
                .unwrap_or_default();
            let start = (checked_datum(player, symbols, &args[1])?
                .int_value()
                .unwrap_or(1)
                .max(1) - 1) as usize;
            let mut end = checked_datum(player, symbols, &args[2])?.int_value().unwrap_or(0) as usize;

            let len = string.chars().count();
            end = end.min(len); // clamp to string length

            if start >= len || end < start + 1 {
                return Ok(player.alloc_datum(Datum::String("".to_string())));
            }

            let substr: String = string.chars().skip(start).take(end - start).collect();

            Ok(player.alloc_datum(Datum::String(substr)))
        })
    }

    pub fn char_to_num(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let str_value = checked_datum(player, symbols, &args[0])?.string_value(symbols)?;
            let mut chars = str_value.chars();

            let byte_val = if let Some(c) = chars.next() {
                c as i32
            } else {
                0
            };

            Ok(player.alloc_datum(Datum::Int(byte_val)))
        })
    }

    pub fn num_to_char(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let num = checked_datum(player, symbols, &args[0])?.int_value()?;
            let byte_val = (num & 0xFF) as u8 as char;

            // Build a single-byte string directly from raw bytes (Latin-1 1:1)
            let result_string = byte_val.to_string();

            Ok(player.alloc_datum(Datum::String(result_string)))
        })
    }

    pub fn url_encode(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            // urlEncode([#empty: sSessionID]) - takes a prop list and converts to URL parameters
            let prop_list_datum = checked_datum(player, symbols, &args[0])?;

            let result = match prop_list_datum {
                Datum::PropList(prop_list, ..) => {
                    // Convert prop list to URL parameters: [#empty: "value"] -> "empty=encoded_value"
                    let mut url_params = String::new();
                    for (key_ref, value_ref) in prop_list {
                        let key = checked_datum(player, symbols, key_ref)?;
                        let value = checked_datum(player, symbols, value_ref)?;

                        let key_str = match key {
                            Datum::Symbol(s) => symbols.display(s).map_err(|_| ScriptError::new("foreign symbol".to_owned()))?,
                            Datum::String(s) => s.as_str(),
                            _ => continue,
                        };

                        let value_str = match value {
                            Datum::String(s) => s.clone(),
                            Datum::Int(n) => n.to_string(),
                            Datum::Float(f) => f.to_string(),
                            Datum::Symbol(s) => symbols.display(s).map_err(|_| ScriptError::new("foreign symbol".to_owned()))?.to_owned(),
                            Datum::Void => String::new(),
                            _ => continue,
                        };

                        if !url_params.is_empty() {
                            url_params.push('&');
                        }

                        // URL encode the value using the same character mapping as ActionScript
                        let mut encoded_value = String::new();
                        for ch in value_str.chars() {
                            let encoded_char = match ch {
                                ':' => "%3A", ';' => "%3B", '<' => "%3C", '=' => "%3D", '>' => "%3E", '?' => "%3F",
                                '@' => "%40", '[' => "%5B", ']' => "%5D", '{' => "%7B", '}' => "%7D", '~' => "%7E",
                                ' ' => "%20", '!' => "%21", '"' => "%22", '#' => "%23", '$' => "%24", '%' => "%25",
                                '&' => "%26", '\'' => "%27", '(' => "%28", ')' => "%29", '*' => "%2A", '+' => "%2B",
                                ',' => "%2C", '-' => "%2D", '.' => "%2E", '/' => "%2F", '©' => "%26%23169", '®' => "%26%23174",
                                _ => {
                                    encoded_value.push(ch);
                                    continue;
                                }
                            };
                            encoded_value.push_str(encoded_char);
                        }

                        url_params.push_str(&format!("{}={}", key_str, encoded_value));
                    }
                    url_params
                },
                Datum::String(s) => {
                    // Direct string encoding (fallback)
                    let mut encoded = String::new();
                    for ch in s.chars() {
                        let encoded_char = match ch {
                            ':' => "%3A", ';' => "%3B", '<' => "%3C", '=' => "%3D", '>' => "%3E", '?' => "%3F",
                            '@' => "%40", '[' => "%5B", ']' => "%5D", '{' => "%7B", '}' => "%7D", '~' => "%7E",
                            ' ' => "%20", '!' => "%21", '"' => "%22", '#' => "%23", '%' => "%25",
                            '&' => "%26", '\'' => "%27", '(' => "%28", ')' => "%29", '*' => "%2A", '+' => "%2B",
                            ',' => "%2C", '©' => "%26%23169", '®' => "%26%23174",
                            _ => {
                                encoded.push(ch);
                                continue;
                            }
                        };
                        encoded.push_str(encoded_char);
                    }
                    encoded
                },
                _ => return Err(ScriptError::new("urlEncode: argument must be a prop list or string".to_string()))
            };

            debug!("urlEncode() = '{}'", result);
            Ok(player.alloc_datum(Datum::String(result)))
        })
    }
}

fn checked_datum<'a>(
    player: &'a crate::player::DirPlayer,
    symbols: &crate::player::symbols::symbol_table::SymbolTable,
    datum_ref: &DatumRef,
) -> Result<&'a Datum, ScriptError> {
    let datum = match datum_ref {
        DatumRef::Void => &Datum::Void,
        _ => player
            .allocator
            .try_get_datum(datum_ref)
            .ok_or_else(|| ScriptError::new_code(
                crate::player::ScriptErrorCode::InvalidReference,
                format!("invalid datum reference {datum_ref}"),
            ))?,
    };
    validate_direct_symbol_fields(datum, symbols)?;
    Ok(datum)
}
