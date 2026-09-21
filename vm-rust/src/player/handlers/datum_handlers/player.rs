use crate::{
    director::lingo::datum::Datum,
    player::{
        DatumRef,
        ScriptError, ScriptErrorCode, session::ExecutionContext,
        symbols::{builtin::BuiltInSymbol, symbol::Symbol},
    },
};
use super::super::types::TypeHandlers;

pub struct PlayerDatumHandlers {}

impl PlayerDatumHandlers {
    pub fn call(runtime: &mut ExecutionContext<'_>, handler_name: Symbol, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        match handler_name.into_builtin() {
            Some(BuiltInSymbol::Count) => Self::count(runtime, args),
            Some(BuiltInSymbol::Cursor) => TypeHandlers::cursor(runtime, args),
            // `_key.keyPressed()` — no-arg form returns the currently-pressed
            // key character (Director 11.5: `_key.keyPressed() = SPACE`); the
            // single-arg form `_key.keyPressed(charOrCode)` tests a specific
            // key and is shared with the top-level `keyPressed()` builtin.
            // parent_dialog's updateDialog (the key-rebind UI) calls the
            // no-arg form: `nn = _key.keyPressed()`.
            Some(BuiltInSymbol::KeyPressed) => {
                if args.is_empty() {
                    runtime.with_player(|player| {
                        let k = player.keyboard_manager.key_pressed();
                        Ok(player.alloc_datum(Datum::String(k)))
                    })
                } else {
                    crate::player::handlers::manager::BuiltInHandlerManager::key_pressed(runtime, args)
                }
            }
            // `_player.getPref(name)` / `_player.setPref(name, value)` — the Player
            // object's form of the top-level getPref()/setPref() (Director 11.5
            // Scripting Dictionary: "Function; retrieves the content of the
            // specified file", VOID when the file doesn't exist, and only .txt /
            // .htm are valid extensions). Same storage either way, so delegate to
            // the existing implementation rather than duplicating it — AreaZero's
            // `[M] Generic Handlers.LoadData` reads its save file with
            // `_player.getPref(gGame.PrefFile)`.
            Some(BuiltInSymbol::GetPref) => Self::get_pref(runtime, args),
            Some(BuiltInSymbol::SetPref) => Self::set_pref(runtime, args),
            // `_player.windowList[1]` — Director 11.5 Scripting Dictionary,
            // `windowList` (Player property, read-only): "displays a list of
            // references to all known movie windows … The Stage is also
            // considered a window." We open no auxiliary MIAWs, so the list is
            // exactly [the Stage], and index 1 is the Stage.
            //
            // AreaZero's `[M] Main.InitGlobals` does
            // `gSystem[#parent] = _player.windowList[1].movie`, which compiles to
            // getPropRef(_player, "windowList", 1) and previously raised.
            Some(BuiltInSymbol::GetProp) | Some(BuiltInSymbol::GetAt) | Some(BuiltInSymbol::GetPropRef) => runtime.with_player_and_symbols(|player, symbols| {
                let subject = player.get_datum(&args[0]).string_value(symbols)?;
                if subject.eq_ignore_ascii_case("windowList") {
                    let index = args.get(1)
                        .map(|a| player.get_datum(a).int_value())
                        .transpose()?
                        .unwrap_or(1);
                    return Ok(if index == 1 {
                        player.alloc_datum(Datum::Stage)
                    } else {
                        DatumRef::Void
                    });
                }
                let handler_text = symbols.display(&handler_name).map_err(|_| {
                    ScriptError::new_code(ScriptErrorCode::InvalidReference, "foreign player handler symbol".to_string())
                })?;
                Err(ScriptError::new(format!(
                    "Invalid call _player.{handler_text}({subject})"
                )))
            }),
            _ => runtime.with_player_and_symbols(|_player, symbols| {
                let handler_text = symbols.display(&handler_name).map_err(|_| {
                    ScriptError::new_code(ScriptErrorCode::InvalidReference, "foreign player handler symbol".to_string())
                })?;
                Err(ScriptError::new_code(
                    ScriptErrorCode::HandlerNotFound,
                    format!("No handler {handler_text} for player datum"),
                ))
            }),
        }
    }

    fn get_pref(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let pref_name = player.get_datum(&args[0]).string_value(symbols)?;
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(value) = player.native_preference(&pref_name).map(str::to_owned) {
                return Ok(player.alloc_datum(Datum::String(value)));
            }
            #[cfg(target_arch = "wasm32")]
            let storage = web_sys::window().and_then(|window| window.local_storage().ok().flatten());
            #[cfg(target_arch = "wasm32")]
            if let Some(storage) = storage {
                let key = format!("dirplayer_pref_{pref_name}");
                if let Ok(Some(value)) = storage.get_item(&key) {
                    return Ok(player.alloc_datum(Datum::String(value)));
                }
            }
            Ok(DatumRef::Void)
        })
    }

    fn set_pref(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let pref_name = player.get_datum(&args[0]).string_value(symbols)?;
            let pref_value = player.get_datum(&args[1]).string_value(symbols)?;
            #[cfg(not(target_arch = "wasm32"))]
            {
                player.set_native_preference(pref_name, pref_value);
                return Ok(DatumRef::Void);
            }
            #[cfg(target_arch = "wasm32")]
            let storage = web_sys::window().and_then(|window| window.local_storage().ok().flatten());
            #[cfg(target_arch = "wasm32")]
            if let Some(storage) = storage {
                let key = format!("dirplayer_pref_{pref_name}");
                let _ = storage.set_item(&key, &pref_value);
            }
            Ok(DatumRef::Void)
        })
    }

    fn count(runtime: &mut ExecutionContext<'_>, args: &Vec<DatumRef>) -> Result<DatumRef, ScriptError> {
        runtime.with_player_and_symbols(|player, symbols| {
            let subject = player.get_datum(&args[0]).string_value(symbols).unwrap();
            // `_player.count(#windowList)` — the symbol arrives as its display
            // spelling, so compare case-insensitively as Director does.
            match_ci!(subject, {
                // EMPTY, matching `the windowList` in movie.rs: dirplayer opens no
                // MIAWs, and movies use a zero count as the "not in a window"
                // signal. Habbo v31 opens with
                //     if _player.windowList.count > 0 then return stopMovie()
                // so a count of 1 refuses to start the movie outright, and
                // Merlin's Revenge accepts mouse input only when
                // `(the windowList).getPos(the frontWindow)` is 0 on both sides.
                //
                // The dictionary does say "The Stage is also considered a
                // window", but that describes what window() operations accept,
                // not what a plugin with no MIAWs enumerates — and two shipping
                // movies read it as empty. Indexing still yields the Stage (see
                // the getProp arm above) so `windowList[1].movie` keeps working.
                "windowList" => Ok(player.alloc_datum(Datum::Int(0))),
                _ => Err(ScriptError::new(
                    format!("Invalid call _player.count({subject})").to_string(),
                ))
            })
        })
    }
}
