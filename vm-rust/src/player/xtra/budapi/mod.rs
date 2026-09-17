//! BudAPI (Buddy API) Xtra — large grab-bag of Windows OS bindings.
//!
//! Most BudAPI handlers are unreachable from a browser sandbox (registry,
//! processes, taskbar, screen saver, wallpaper, printers, …). The port here
//! exposes every documented entry point so Lingo scripts written against
//! BudAPI don't blow up with "no built-in handler", returns the correct
//! WASM-environment answer where one is meaningful (clipboard, locale,
//! screen size, base64, key-state, sleep, alert, open URL, encrypt /
//! decrypt, font list, system time, environment, file ops via FileIO's
//! virtual filesystem), and returns the documented sentinel (`""` /
//! `0` / `-1`) for things that genuinely don't exist in a browser.

use base64::Engine;

use crate::{
    director::lingo::datum::{Datum, DatumType},
    player::{
        symbols::symbol_table::SymbolTable, DatumRef, DirPlayer,
        ScriptError,
    },
};

const BUDAPI_VERSION: &str = "5.0";

/// Mutable BudAPI settings belong to one player generation.  Keeping them in
/// the Xtra manager prevents one browser player from changing another
/// player's keyboard, mouse, screen-saver, or volume state.
#[derive(Clone, Debug)]
pub(crate) struct BudApiState {
    pub(crate) mouse_disabled: bool,
    pub(crate) keys_disabled: bool,
    pub(crate) screensaver_disabled: bool,
    pub(crate) sound_volume: u8,
    /// Browser clipboard fallback for this player generation.  It must not
    /// use shared DOM storage: two players may have different clipboard
    /// fixtures and reset must clear the retired generation's value.
    pub(crate) clipboard_text: String,
}

#[derive(Clone, Debug)]
pub(crate) enum BudApiHostIntent {
    Open { owner: crate::player::ownership::OwnerToken, target: String },
    Alert { owner: crate::player::ownership::OwnerToken, text: String },
    ClipboardWrite { owner: crate::player::ownership::OwnerToken, text: String },
    ClipboardRead { owner: crate::player::ownership::OwnerToken },
}

impl BudApiHostIntent {
    pub(crate) fn owner(&self) -> &crate::player::ownership::OwnerToken {
        match self {
            Self::Open { owner, .. }
            | Self::Alert { owner, .. }
            | Self::ClipboardWrite { owner, .. }
            | Self::ClipboardRead { owner } => owner,
        }
    }
}

impl Default for BudApiState {
    fn default() -> Self {
        Self {
            mouse_disabled: false,
            keys_disabled: false,
            screensaver_disabled: false,
            sound_volume: 100,
            clipboard_text: String::new(),
        }
    }
}

pub struct BudApiXtra;

impl BudApiXtra {
    pub fn has_handler(name: &str) -> bool {
        // BudAPI functions all start with the lowercase "ba" prefix.
        let lower = name.to_ascii_lowercase();
        lower.starts_with("ba")
            && match lower.as_str() {
                // Excludes a couple of built-in Director handlers that happen
                // to begin with "ba" but aren't BudAPI — none currently in
                // dirplayer-rs, but keeps the door open.
                _ => true,
            }
    }

    pub fn call_handler(
        player: &mut DirPlayer,
        state: &mut BudApiState,
        name: &str,
        args: &Vec<DatumRef>,
        symbols: &SymbolTable,
    ) -> Result<DatumRef, ScriptError> {
        dispatch(player, state, name, args, symbols)
    }

    pub(crate) fn prepare_handler(
        player: &mut DirPlayer,
        state: &mut BudApiState,
        symbols: &SymbolTable,
        name: &str,
        args: &[DatumRef],
    ) -> Result<crate::player::xtra::manager::XtraPendingOrValue, ScriptError> {
        let owner = player.owner.clone();
        let host = match name.to_ascii_lowercase().as_str() {
            "barunprogram" | "bashell" | "baopenfile" | "baopenurl" => {
                Some(BudApiHostIntent::Open {
                    owner,
                    target: string_arg(player, &args.to_vec(), 0, symbols)?,
                })
            }
            "bamsgbox" | "bamsgboxex" => {
                let message = string_arg(player, &args.to_vec(), 0, symbols)?;
                let caption = if args.len() > 1 {
                    string_arg(player, &args.to_vec(), 1, symbols)?
                } else {
                    String::new()
                };
                let text = if caption.is_empty() { message } else { format!("{}\n\n{}", caption, message) };
                Some(BudApiHostIntent::Alert { owner, text })
            }
            "bacopytext" => Some(BudApiHostIntent::ClipboardWrite {
                owner,
                text: string_arg(player, &args.to_vec(), 0, symbols)?,
            }),
            "bapastetext" => Some(BudApiHostIntent::ClipboardRead { owner }),
            _ => None,
        };
        if let Some(host) = host {
            return Ok(crate::player::xtra::manager::XtraPendingOrValue::Pending(
                crate::player::xtra::manager::XtraPendingIntent::BudApi(host),
            ));
        }
        dispatch(player, state, name, &args.to_vec(), symbols)
            .map(crate::player::xtra::manager::XtraPendingOrValue::Value)
    }
}

fn dispatch(
    player: &mut DirPlayer,
    state: &mut BudApiState,
    name: &str,
    args: &Vec<DatumRef>,
    symbols: &SymbolTable,
) -> Result<DatumRef, ScriptError> {
    match_ci!(name, {
        // -- Information ------------------------------------------------
        "baVersion" => ok_string(player, BUDAPI_VERSION),
        "baSysFolder" => ba_sys_folder(player, args, symbols),
        "baCpuInfo" => ba_cpu_info(player, args, symbols),
        "baDiskInfo" => ok_int(player, -1),
        "baDiskList" => empty_list(player, ),
        "baMemoryInfo" => ok_int(player, 0),
        "baFindApp" => ok_string(player, ""),
        "baReadIni" | "baWriteIni" | "baDeleteIniEntry" | "baDeleteIniSection" | "baFlushIni" => ok_int(player, 0),
        "baReadRegString" | "baReadRegMulti" | "baReadRegBinary" => default_string(player, args, symbols),
        "baReadRegNumber" => default_int(player, args, symbols),
        "baWriteRegString" | "baWriteRegNumber" | "baWriteRegBinary" | "baWriteRegMulti" | "baDeleteReg" => ok_int(player, 0),
        "baRegKeyList" | "baRegValueList" => empty_list(player, ),
        "baSoundCard" => ok_int(player, 1),
        "baFontInstalled" => ba_font_installed(player, args, symbols),
        "baFontList" => ba_font_list(player, args, symbols),
        "baFontStyleList" => empty_list(player, ),
        "baCommandArgs" => ok_string(player, ""),
        "baPrevious" => ok_int(player, 0),
        "baScreenInfo" => ba_screen_info(player, args, symbols),

        // -- System -----------------------------------------------------
        "baDisableDiskErrors" => ok_int(player, 0),
        "baDisableKeys" => { state.keys_disabled = int_arg_or(player, args, 0, 0, symbols)? != 0; ok_int(player, 0) },
        "baDisableMouse" => { state.mouse_disabled = int_arg_or(player, args, 0, 0, symbols)? != 0; ok_int(player, 0) },
        "baDisableSwitching" => ok_int(player, 0),
        "baDisableScreenSaver" => { state.screensaver_disabled = int_arg_or(player, args, 0, 0, symbols)? != 0; ok_int(player, 0) },
        "baScreenSaverTime" | "baSetScreenSaver" | "baSetWallpaper" | "baSetPattern"
            | "baSetDisplay" | "baSetDisplayEx" | "baExitWindows" | "baWinHelp"
            | "baHideTaskBar" | "baSetCurrentDir" | "baPlaceCursor" | "baRestrictCursor"
            | "baFreeCursor" | "baSetSystemTime" | "baEjectDisk" | "baInstallFont"
            | "baCreatePMGroup" | "baDeletePMGroup" | "baCreatePMIcon" | "baDeletePMIcon"
            | "baRefreshDesktop" | "baSetPrinter" | "baPrintDlg" | "baPageSetupDlg" => ok_int(player, 0),
        "baRunProgram" | "baShell" | "baMsgBox" | "baMsgBoxEx" | "baCopyText" =>
            Err(ScriptError::new("BudAPI browser effect requires the owner host executor".to_owned())),
        "baPasteText" => Err(ScriptError::new("BudAPI clipboard read requires the owner host executor".to_owned())),
        "baEncryptText" => ba_encrypt_text(player, args, symbols),
        "baDecryptText" => ba_decrypt_text(player, args, symbols),
        "baSetVolume" => { let v = int_arg_or(player, args, 1, 100, symbols)?.clamp(0, 255) as u8; state.sound_volume = v; ok_int(player, 0) },
        "baGetVolume" => ok_int(player, state.sound_volume as i32),
        "baEnvironment" => ba_environment(player, args, symbols),
        "baSetEnvironment" => ok_int(player, 0),
        "baAdministrator" => ok_int(player, 0),
        "baUserName" | "baComputerName" => ok_string(player, ""),
        "baKeyIsDown" | "baKeyBeenPressed" => ok_int(player, 0),
        "baSleep" => ba_sleep(player, args, symbols),
        "baPMGroupList" | "baPMIconList" | "baPMSubGroupList" => empty_list(player, ),
        "baSystemTime" => ba_system_time(player, args, symbols),
        "baPrinterInfo" => ok_string(player, ""),

        // -- File -------------------------------------------------------
        "baFileExists" => ba_file_exists(player, args, symbols),
        "baFolderExists" => ba_folder_exists(player, args, symbols),
        "baFileSize" => ba_file_size(player, args, symbols),
        "baCreateFolder" | "baDeleteFolder" | "baRenameFile" | "baDeleteFile"
            | "baDeleteXFiles" | "baXDelete" | "baSetFileDate" | "baSetFileAttributes"
            | "baRecycleFile" | "baCopyFile" | "baCopyXFiles" | "baXCopy" | "baMakeShortcut"
            | "baMakeShortcutEx" | "baFindClose" => ok_int(player, 0),
        "baFileAge" => ok_int(player, -1),
        "baFileDate" | "baFileDateEx" => ok_string(player, ""),
        "baFileAttributes" => ok_string(player, ""),
        "baFileList" | "baFolderList" => ba_file_list(player, args, symbols),
        "baFindFirstFile" | "baFindNextFile" => ok_string(player, ""),
        "baGetFilename" | "baGetFolder" => ok_string(player, ""),
        "baFileVersion" => ok_string(player, ""),
        "baEncryptFile" => ok_int(player, 0),
        "baFindDrive" => ok_string(player, ""),
        "baOpenFile" | "baOpenURL" => Err(ScriptError::new("BudAPI browser effect requires the owner host executor".to_owned())),
        "baPrintFile" => ok_int(player, 0),
        "baShortFileName" | "baLongFileName" => default_string(player, args, symbols),
        "baTempFileName" => ba_temp_file_name(player, args, symbols),
        "baResolveShortcut" => default_string(player, args, symbols),

        // -- Window functions (all browser no-ops) ----------------------
        "baWindowInfo" => ok_string(player, ""),
        "baFindWindow" | "baActiveWindow" | "baWinHandle" | "baStageHandle"
            | "baActivateWindow" | "baCloseWindow" | "baCloseApp" | "baSetWindowState"
            | "baSetWindowTitle" | "baMoveWindow" | "baWindowToFront" | "baWindowToBack"
            | "baGetWindow" | "baWaitTillActive" | "baWaitForWindow" | "baNextActiveWindow"
            | "baWindowExists" | "baWindowDepth" | "baSetWindowDepth" | "baSendKeys"
            | "baSendMsg" | "baAddSysItems" | "baRemoveSysItems" | "baClipWindow"
            | "baSetParent" => ok_int(player, 0),
        "baWindowList" | "baChildWindowList" => empty_list(player, ),

        // -- Buddy meta -------------------------------------------------
        "baAbout" => ok_int(player, 0),
        "baRegister" | "baSaveRegistration" => ok_int(player, 1),
        "baGetRegistration" => ok_string(player, ""),
        "baFunctions" => ok_int(player, i32::MAX),
        "baUsedFunctions" => empty_list(player, ),

        _ => {
            log::warn!("[BudAPI] unhandled handler: {}", name);
            ok_int(player, 0)
        },
    })
}

// -- Helpers ----------------------------------------------------------------

fn ok_int(player: &mut DirPlayer, n: i32) -> Result<DatumRef, ScriptError> {
    Ok(player.alloc_datum(Datum::Int(n)))
}

fn ok_string(player: &mut DirPlayer, s: &str) -> Result<DatumRef, ScriptError> {
    Ok(player.alloc_datum(Datum::String(s.to_owned())))
}

fn empty_list(player: &mut DirPlayer) -> Result<DatumRef, ScriptError> {
    Ok(player.alloc_datum(Datum::List(
        DatumType::List,
        std::collections::VecDeque::new(),
        false,
    )))
}

fn int_arg_or(
    player: &DirPlayer,
    args: &Vec<DatumRef>,
    idx: usize,
    default: i32,
    _symbols: &SymbolTable,
) -> Result<i32, ScriptError> {
    match args.get(idx) {
        Some(a) => checked_datum(player, a)?.int_value(),
        None => Ok(default),
    }
}

fn string_arg_explicit(
    player: &DirPlayer,
    args: &Vec<DatumRef>,
    idx: usize,
    symbols: &SymbolTable,
) -> Result<String, ScriptError> {
    match args.get(idx) {
        Some(a) => checked_datum(player, a)?.string_value(symbols),
        None => Ok(String::new()),
    }
}

fn checked_datum<'a>(player: &'a DirPlayer, value: &DatumRef) -> Result<&'a Datum, ScriptError> {
    match value {
        DatumRef::Void => Ok(&Datum::Void),
        _ => player.allocator.try_get_datum(value).ok_or_else(|| {
            ScriptError::new_code(
                crate::player::ScriptErrorCode::InvalidReference,
                format!("foreign or stale BudAPI datum reference {value}"),
            )
        }),
    }
}

fn string_arg(
    player: &DirPlayer,
    args: &Vec<DatumRef>,
    idx: usize,
    symbols: &SymbolTable,
) -> Result<String, ScriptError> {
    match args.get(idx) {
        Some(a) => checked_datum(player, a)?.string_value(symbols),
        None => Ok(String::new()),
    }
}

/// Many BudAPI getters accept a default-value argument that's returned
/// verbatim when the underlying read fails. In WASM the read effectively
/// always fails, so we just echo the default back. Default is in arg[2] for
/// baReadRegString-style calls, and arg[0] for shortname-style getters.
fn default_string(player: &mut DirPlayer, args: &Vec<DatumRef>, symbols: &SymbolTable) -> Result<DatumRef, ScriptError> {
    let default_idx = if args.len() >= 3 { 2 } else { 0 };
    let s = string_arg(player, args, default_idx, symbols)?;
    Ok(player.alloc_datum(Datum::String(s)))
}

fn default_int(player: &mut DirPlayer, args: &Vec<DatumRef>, symbols: &SymbolTable) -> Result<DatumRef, ScriptError> {
    let n = int_arg_or(player, args, 2, 0, symbols)?;
    Ok(player.alloc_datum(Datum::Int(n)))
}

// -- Information ------------------------------------------------------------

fn ba_sys_folder(player: &mut DirPlayer, args: &Vec<DatumRef>, symbols: &SymbolTable) -> Result<DatumRef, ScriptError> {
    let kind = string_arg(player, args, 0, symbols)?;
    let path = match kind.to_ascii_lowercase().as_str() {
        "temp" | "windows" | "system" | "program files" | "appdata" | "localappdata" => "/",
        _ => "/",
    };
    ok_string(player, path)
}

fn ba_cpu_info(player: &mut DirPlayer, args: &Vec<DatumRef>, symbols: &SymbolTable) -> Result<DatumRef, ScriptError> {
    let kind = string_arg(player, args, 0, symbols)?;
    let value = match kind.to_ascii_lowercase().as_str() {
        "vendor" => "WebAssembly",
        "name" => "WASM Virtual CPU",
        "speed" => "0",
        "cores" => web_sys::window()
            .and_then(|w| w.navigator().hardware_concurrency().to_string().into())
            .map(|_| "")
            .unwrap_or(""),
        _ => "",
    };
    ok_string(player, value)
}

fn ba_screen_info(player: &mut DirPlayer, args: &Vec<DatumRef>, symbols: &SymbolTable) -> Result<DatumRef, ScriptError> {
    let kind = string_arg(player, args, 0, symbols)?;
    let screen = web_sys::window().and_then(|w| w.screen().ok());
    let result = match kind.to_ascii_lowercase().as_str() {
        "width" => screen.as_ref().and_then(|s| s.width().ok()).unwrap_or(0),
        "height" => screen.as_ref().and_then(|s| s.height().ok()).unwrap_or(0),
        "depth" | "colordepth" => screen
            .as_ref()
            .and_then(|s| s.color_depth().ok())
            .unwrap_or(24),
        "pixeldepth" => screen
            .as_ref()
            .and_then(|s| s.pixel_depth().ok())
            .unwrap_or(24),
        _ => 0,
    };
    ok_int(player, result)
}

fn ba_font_installed(player: &mut DirPlayer, args: &Vec<DatumRef>, symbols: &SymbolTable) -> Result<DatumRef, ScriptError> {
    let name = string_arg(player, args, 0, symbols)?;
    if name.is_empty() {
        return ok_int(player, 0);
    }
    // Canvas-based font detection: measure a probe string twice using two
    // distinct fallback families plus the candidate. If both widths still
    // match the fallbacks, the candidate isn't actually installed.
    let document = match web_sys::window().and_then(|w| w.document()) {
        Some(d) => d,
        None => return ok_int(player, 0),
    };
    let canvas = match document.create_element("canvas") {
        Ok(el) => el.dyn_into::<web_sys::HtmlCanvasElement>().ok(),
        Err(_) => None,
    };
    let canvas = match canvas {
        Some(c) => c,
        None => return ok_int(player, 0),
    };
    let ctx_obj = match canvas.get_context("2d") {
        Ok(Some(c)) => c,
        _ => return ok_int(player, 0),
    };
    let ctx: web_sys::CanvasRenderingContext2d = match ctx_obj.dyn_into() {
        Ok(c) => c,
        Err(_) => return ok_int(player, 0),
    };
    let probe = "mwjxyzABCabc012345";
    let measure = |font: &str| -> f64 {
        ctx.set_font(font);
        ctx.measure_text(probe).map(|m| m.width()).unwrap_or(0.0)
    };
    let baseline_a = measure("72px monospace");
    let baseline_b = measure("72px serif");
    let candidate_a = measure(&format!("72px '{}', monospace", name));
    let candidate_b = measure(&format!("72px '{}', serif", name));
    let installed =
        (candidate_a - baseline_a).abs() > 0.5 || (candidate_b - baseline_b).abs() > 0.5;
    ok_int(player, if installed { 1 } else { 0 })
}

fn ba_font_list(player: &mut DirPlayer, args: &Vec<DatumRef>, symbols: &SymbolTable) -> Result<DatumRef, ScriptError> {
    // Browsers don't expose a font enumeration API on the open web (FontFace
    // API is privacy-gated). Always return the canonical web-safe set.
    let _ = string_arg(player, args, 0, symbols)?;
    let names = [
        "Arial",
        "Arial Black",
        "Comic Sans MS",
        "Courier New",
        "Georgia",
        "Helvetica",
        "Impact",
        "Tahoma",
        "Times New Roman",
        "Trebuchet MS",
        "Verdana",
    ];
    let refs: std::collections::VecDeque<DatumRef> = names
        .iter()
        .map(|n| player.alloc_datum(Datum::String(n.to_string())))
        .collect();
    Ok(player.alloc_datum(Datum::List(DatumType::List, refs, false)))
}

use wasm_bindgen::JsCast;

// -- System / clipboard / time ---------------------------------------------

fn ba_environment(player: &mut DirPlayer, args: &Vec<DatumRef>, symbols: &SymbolTable) -> Result<DatumRef, ScriptError> {
    let name = string_arg(player, args, 0, symbols)?;
    let value = match name.to_ascii_uppercase().as_str() {
        "USERLANGUAGE" | "LANG" => web_sys::window()
            .map(|w| w.navigator().language().unwrap_or_default())
            .unwrap_or_default(),
        "USERAGENT" => web_sys::window()
            .and_then(|w| w.navigator().user_agent().ok())
            .unwrap_or_default(),
        _ => String::new(),
    };
    ok_string(player, &value)
}

fn ba_sleep(player: &mut DirPlayer, args: &Vec<DatumRef>, symbols: &SymbolTable) -> Result<DatumRef, ScriptError> {
    // We can't block the WASM thread; busy-wait `Date.now()` instead so
    // callers get the time delay they asked for (without making the page
    // unresponsive — we cap at 500ms to avoid runaway scripts).
    let ms = int_arg_or(player, args, 0, 0, symbols)?.clamp(0, 500) as f64;
    if let Some(perf) = web_sys::window().and_then(|w| w.performance()) {
        let end = perf.now() + ms;
        while perf.now() < end {
            // tight loop, but ≤500ms by clamp above
        }
    }
    ok_int(player, 0)
}

fn ba_system_time(player: &mut DirPlayer, args: &Vec<DatumRef>, symbols: &SymbolTable) -> Result<DatumRef, ScriptError> {
    use chrono::{Datelike, Local, Timelike};
    let format = string_arg(player, args, 0, symbols)?;
    let now = Local::now();
    let formatted = match format.to_ascii_uppercase().as_str() {
        "" | "LONG" => now.format("%A, %B %e, %Y %H:%M:%S").to_string(),
        "SHORT" | "DATE" => now.format("%m/%d/%Y").to_string(),
        "TIME" => now.format("%H:%M:%S").to_string(),
        "ISO" => now.format("%Y-%m-%dT%H:%M:%S").to_string(),
        "DAYOFWEEK" => now.weekday().num_days_from_sunday().to_string(),
        "YEAR" => now.year().to_string(),
        "MONTH" => now.month().to_string(),
        "DAY" => now.day().to_string(),
        "HOUR" => now.hour().to_string(),
        "MINUTE" => now.minute().to_string(),
        "SECOND" => now.second().to_string(),
        _ => now.format(&format).to_string(),
    };
    ok_string(player, &formatted)
}

// -- File ops via FileIO virtual filesystem --------------------------------

fn ba_file_exists(player: &mut DirPlayer, args: &Vec<DatumRef>, symbols: &SymbolTable) -> Result<DatumRef, ScriptError> {
    let path = string_arg_explicit(player, args, 0, symbols)?;
    let exists = player.with_xtra_manager_state(|state, _| {
        state.fileio.virtual_fs.contains_key(&path)
            || state.fileio.virtual_fs.contains_key(path.trim_start_matches('/'))
    });
    Ok(player.alloc_datum(Datum::Int(if exists { 1 } else { 0 })))
}

fn ba_folder_exists(player: &mut DirPlayer, args: &Vec<DatumRef>, symbols: &SymbolTable) -> Result<DatumRef, ScriptError> {
    let path = string_arg_explicit(player, args, 0, symbols)?;
    let prefix = if path.ends_with('/') { path } else { format!("{}/", path) };
    let exists = player.with_xtra_manager_state(|state, _| {
        state.fileio.virtual_fs.keys().any(|k| k.starts_with(&prefix))
    });
    Ok(player.alloc_datum(Datum::Int(if exists { 1 } else { 0 })))
}

fn ba_file_size(player: &mut DirPlayer, args: &Vec<DatumRef>, symbols: &SymbolTable) -> Result<DatumRef, ScriptError> {
    let path = string_arg_explicit(player, args, 0, symbols)?;
    let size = player.with_xtra_manager_state(|state, _| {
        state.fileio.virtual_fs.get(&path)
            .or_else(|| state.fileio.virtual_fs.get(path.trim_start_matches('/')))
            .map(|d| d.len() as i32).unwrap_or(-1)
    });
    Ok(player.alloc_datum(Datum::Int(size)))
}

fn ba_file_list(player: &mut DirPlayer, args: &Vec<DatumRef>, symbols: &SymbolTable) -> Result<DatumRef, ScriptError> {
    let folder = string_arg_explicit(player, args, 0, symbols)?;
    let _pattern = string_arg_explicit(player, args, 1, symbols)?;
    let prefix = if folder.is_empty() { String::new() } else if folder.ends_with('/') { folder } else { format!("{}/", folder) };
    let files = player.with_xtra_manager_state(|state, _| {
        state.fileio.virtual_fs.keys().filter(|k| k.starts_with(&prefix)).cloned().collect::<Vec<_>>()
    });
    let refs: std::collections::VecDeque<DatumRef> = files.into_iter().map(|f| player.alloc_datum(Datum::String(f))).collect();
    Ok(player.alloc_datum(Datum::List(DatumType::List, refs, false)))
}

fn ba_temp_file_name(player: &mut DirPlayer, args: &Vec<DatumRef>, symbols: &SymbolTable) -> Result<DatumRef, ScriptError> {
    let prefix = string_arg(player, args, 0, symbols)?;
    let mut raw = [0u8; 8];
    let _ = getrandom::fill(&mut raw);
    let suffix: String = raw.iter().map(|b| format!("{:02x}", b)).collect();
    ok_string(player, &format!("/tmp/{}{}.tmp", prefix, suffix))
}

// -- Encrypt / decrypt -----------------------------------------------------

/// Buddy API's baEncryptText / baDecryptText use a simple key-XOR cipher;
/// the exact algorithm isn't documented but the standard port is "repeat
/// the key bytes across the plaintext and XOR, then base64-encode the
/// result" (and decrypt reverses). We implement that here so encrypt and
/// decrypt round-trip even if a server hasn't been reverse-engineered.
fn xor_with_key(data: &[u8], key: &[u8]) -> Vec<u8> {
    if key.is_empty() {
        return data.to_vec();
    }
    data.iter()
        .enumerate()
        .map(|(i, b)| b ^ key[i % key.len()])
        .collect()
}

fn ba_encrypt_text(player: &mut DirPlayer, args: &Vec<DatumRef>, symbols: &SymbolTable) -> Result<DatumRef, ScriptError> {
    let text = string_arg(player, args, 0, symbols)?;
    let key = string_arg(player, args, 1, symbols)?;
    let bytes: Vec<u8> = text.chars().map(|c| c as u8).collect();
    let key_bytes: Vec<u8> = key.chars().map(|c| c as u8).collect();
    let cipher = xor_with_key(&bytes, &key_bytes);
    let encoded = base64::engine::general_purpose::STANDARD.encode(cipher);
    ok_string(player, &encoded)
}

fn ba_decrypt_text(player: &mut DirPlayer, args: &Vec<DatumRef>, symbols: &SymbolTable) -> Result<DatumRef, ScriptError> {
    let text = string_arg(player, args, 0, symbols)?;
    let key = string_arg(player, args, 1, symbols)?;
    let cipher = match base64::engine::general_purpose::STANDARD.decode(text.as_bytes()) {
        Ok(v) => v,
        Err(_) => return ok_string(player, ""),
    };
    let key_bytes: Vec<u8> = key.chars().map(|c| c as u8).collect();
    let plain = xor_with_key(&cipher, &key_bytes);
    let s: String = plain.iter().map(|&b| b as char).collect();
    ok_string(player, &s)
}
