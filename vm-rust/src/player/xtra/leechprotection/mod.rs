//! LeechProtectionRemovalHelp Xtra 1.6.0 (Anthony Kleine).
//!
//! The original Xtra exists to make an archived Shockwave movie believe it is
//! still running from its original home: it fakes `the moviePath`, `the
//! movieName`, `the environment`, the external parameters, and it pins
//! `the exitLock` / `the safePlayer` so a movie's own leech check can't undo
//! them. Its README states the values "stay set, forced, disabled or bugfixed
//! even after going to other movies" — that persistence is the whole point,
//! since the check usually lives in a *later* movie loaded by `gotoNetMovie`.
//!
//! The Windows implementation achieves this by hot-patching Director itself —
//! `Script.cpp` is ~7000 lines of `__declspec(naked)` trampolines with a
//! separate set of hook addresses per Director build (8.0 through 12.0),
//! reaching into `dirapi.dll`, `netlingo.x32` and the Shockwave 3D Asset Xtra.
//! None of that is portable, and none of it is necessary here: dirplayer *is*
//! the runtime, so we implement the resulting semantics directly against
//! [`EnvOverrides`], which lives on the `Player` (not the `Movie`) and so
//! naturally outlives a movie load — the same contract the asm hooks buy.
//!
//! Message table (all entries are `*`-prefixed, i.e. global handlers; `new`
//! exists only so `new(xtra "LeechProtectionRemovalHelp")` succeeds):
//!
//! ```text
//! new object me
//! * setTheMoviePath string moviePath
//! * setTheMovieName string movieName
//! * setTheEnvironment_shockMachine integer environment_shockMachine
//! * setTheEnvironment_shockMachineVersion string environment_shockMachineVersion
//! * setThePlatform string platform
//! * setTheRunMode string runMode
//! * setTheEnvironment_productBuildVersion string environment_productBuildVersion
//! * setTheProductVersion string productVersion
//! * setTheEnvironment_osVersion string environment_osVersion
//! * setTheMachineType integer machineType
//! * setExternalParam string name, string value
//! * forceTheExitLock integer exitLock
//! * forceTheSafePlayer integer safePlayer
//! * disableGoToNetMovie
//! * disableGoToNetPage
//! * bugfixShockwave3DBadDriverList
//! ```
//!
//! Every handler returns VOID — `TStdXtra_IMoaMmXScript::Call` dispatches on
//! the selector and never writes `callPtr->resultValue`.

use crate::player::{
    driver::checked_internal_datum, symbols::symbol_table::SymbolTable, DatumRef, DirPlayer,
    ScriptError,
};

/// Fake environment installed by the LeechProtectionRemovalHelp Xtra.
///
/// `None` means "report dirplayer's own value". Stored on the `Player` so the
/// overrides survive `gotoNetMovie` / `go to movie`, matching the Xtra's
/// documented behaviour.
#[derive(Clone, Debug, Default)]
pub struct EnvOverrides {
    /// `the moviePath`, `the path`, `the pathName`, `_movie.path`.
    pub movie_path: Option<String>,
    /// `the movieName`, `the movie`, `_movie.name`.
    pub movie_name: Option<String>,
    /// `the environment.shockMachine` (and the propList / `_system` forms).
    pub shock_machine: Option<i32>,
    /// `the environment.shockMachineVersion`.
    pub shock_machine_version: Option<String>,
    /// `the platform` and `the environment.platform`.
    pub platform: Option<String>,
    /// `the runMode`, `the environment.runMode`, `_player.runMode`.
    pub run_mode: Option<String>,
    /// `the environment.productBuildVersion`.
    pub product_build_version: Option<String>,
    /// `the productVersion`, `the environment.productVersion`,
    /// `_player.productVersion`.
    pub product_version: Option<String>,
    /// `the environment.osVersion`.
    pub os_version: Option<String>,
    /// `the machineType`.
    pub machine_type: Option<i32>,
    /// `the exitLock`, pinned: reads return this and movie writes are dropped.
    pub forced_exit_lock: Option<bool>,
    /// `the safePlayer`, pinned the same way.
    pub forced_safe_player: Option<bool>,
    /// `gotoNetMovie` becomes a no-op.
    pub disable_goto_net_movie: bool,
    /// `gotoNetPage` becomes a no-op.
    pub disable_goto_net_page: bool,
}

pub struct LeechProtectionXtra;

impl LeechProtectionXtra {
    pub fn has_handler(name: &str) -> bool {
        matches!(
            name.to_ascii_lowercase().as_str(),
            "setthemoviepath"
                | "setthemoviename"
                | "settheenvironment_shockmachine"
                | "settheenvironment_shockmachineversion"
                | "settheplatform"
                | "settherunmode"
                | "settheenvironment_productbuildversion"
                | "settheproductversion"
                | "settheenvironment_osversion"
                | "setthemachinetype"
                | "setexternalparam"
                | "forcetheexitlock"
                | "forcethesafeplayer"
                | "disablegotonetmovie"
                | "disablegotonetpage"
                | "bugfixshockwave3dbaddriverlist"
        )
    }

    /// Owner-bound static dispatch. The Xtra mutates player-level overrides,
    /// so the player and its session symbol table must travel together through
    /// this call; no ambient player is consulted.
    pub(crate) fn call_handler_explicit(
        player: &mut DirPlayer,
        name: &str,
        args: &[DatumRef],
        symbols: &mut SymbolTable,
    ) -> Result<DatumRef, ScriptError> {
        match_ci!(name, {
            "setTheMoviePath" => set_string_explicit(player, name, args, symbols, |o, v| o.movie_path = Some(v)),
            "setTheMovieName" => set_string_explicit(player, name, args, symbols, |o, v| o.movie_name = Some(v)),
            "setTheEnvironment_shockMachine" => set_int_explicit(player, name, args, symbols, |o, v| o.shock_machine = Some(v)),
            "setTheEnvironment_shockMachineVersion" => set_string_explicit(player, name, args, symbols, |o, v| o.shock_machine_version = Some(v)),
            "setThePlatform" => set_string_explicit(player, name, args, symbols, |o, v| o.platform = Some(v)),
            "setTheRunMode" => set_string_explicit(player, name, args, symbols, |o, v| o.run_mode = Some(v)),
            "setTheEnvironment_productBuildVersion" => set_string_explicit(player, name, args, symbols, |o, v| o.product_build_version = Some(v)),
            "setTheProductVersion" => set_string_explicit(player, name, args, symbols, |o, v| o.product_version = Some(v)),
            "setTheEnvironment_osVersion" => set_string_explicit(player, name, args, symbols, |o, v| o.os_version = Some(v)),
            "setTheMachineType" => set_int_explicit(player, name, args, symbols, |o, v| o.machine_type = Some(v)),
            "setExternalParam" => set_external_param_explicit(player, args, symbols),
            "forceTheExitLock" => set_int_explicit(player, name, args, symbols, |o, v| o.forced_exit_lock = Some(v != 0)),
            "forceTheSafePlayer" => set_int_explicit(player, name, args, symbols, |o, v| o.forced_safe_player = Some(v != 0)),
            "disableGoToNetMovie" => { player.env_overrides.disable_goto_net_movie = true; Ok(DatumRef::Void) },
            "disableGoToNetPage" => { player.env_overrides.disable_goto_net_page = true; Ok(DatumRef::Void) },
            "bugfixShockwave3DBadDriverList" => Ok(DatumRef::Void),
            _ => Err(ScriptError::new(format!(
                "LeechProtectionRemovalHelp: no handler {}",
                name
            ))),
        })
    }
}

fn set_string_explicit(
    player: &mut DirPlayer,
    name: &str,
    args: &[DatumRef],
    symbols: &SymbolTable,
    apply: impl FnOnce(&mut EnvOverrides, String),
) -> Result<DatumRef, ScriptError> {
    let value = args
        .first()
        .ok_or_else(|| {
            ScriptError::new(format!(
                "LeechProtectionRemovalHelp: {} requires 1 argument",
                name
            ))
        })
        .and_then(|arg| checked_internal_datum(player, symbols, arg)?.string_value(symbols))?;
    apply(&mut player.env_overrides, value);
    Ok(DatumRef::Void)
}

fn set_int_explicit(
    player: &mut DirPlayer,
    name: &str,
    args: &[DatumRef],
    symbols: &SymbolTable,
    apply: impl FnOnce(&mut EnvOverrides, i32),
) -> Result<DatumRef, ScriptError> {
    let value = args
        .first()
        .ok_or_else(|| {
            ScriptError::new(format!(
                "LeechProtectionRemovalHelp: {} requires 1 argument",
                name
            ))
        })
        .and_then(|arg| checked_internal_datum(player, symbols, arg)?.int_value())?;
    apply(&mut player.env_overrides, value);
    Ok(DatumRef::Void)
}

fn set_external_param_explicit(
    player: &mut DirPlayer,
    args: &[DatumRef],
    symbols: &SymbolTable,
) -> Result<DatumRef, ScriptError> {
    let name = args
        .first()
        .ok_or_else(|| {
            ScriptError::new(
                "LeechProtectionRemovalHelp: setExternalParam requires a name".to_owned(),
            )
        })
        .and_then(|arg| checked_internal_datum(player, symbols, arg)?.string_value(symbols))?;
    if name.is_empty() {
        return Ok(DatumRef::Void);
    }
    let value = args
        .get(1)
        .map(|arg| checked_internal_datum(player, symbols, arg)?.string_value(symbols))
        .transpose()?
        .unwrap_or_default();
    let existing = player
        .external_params
        .keys()
        .find(|key| key.eq_ignore_ascii_case(&name))
        .cloned();
    player
        .external_params
        .insert(existing.unwrap_or(name), value);
    Ok(DatumRef::Void)
}

/// `setExternalParam name, value` — writes straight into the player's external
/// parameter map, which `externalParamName` / `externalParamValue` /
/// `externalParamCount` already read case-insensitively. The map is an
/// `IndexMap`, so a param added here appends at the end and keeps a stable
/// index for the indexed accessors; re-setting an existing name updates it in
/// place without moving it.
///
/// The Xtra's own message table documents "name must not be empty"; an empty
/// name is silently ignored.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    //! Run with:
    //!   cargo test --lib --manifest-path vm-rust/Cargo.toml leechprotection

    use crate::director::lingo::datum::Datum;
    use crate::player::session::{RuntimeSession, RuntimeSessionHandle};
    use crate::player::symbols::symbol_table::SymbolOwner;
    use crate::player::symbols::{builtin::BuiltInSymbol, symbol::Symbol};
    use crate::player::testing::run_test;
    use crate::player::xtra::manager::try_call_xtra_static_handler_explicit;
    use crate::player::{DatumRef, ScriptError};

    /// Call an LPRH handler the way a movie would — through the static
    /// dispatcher, so the manager wiring is under test too.
    fn test_session() -> RuntimeSessionHandle {
        let session = RuntimeSession::new(SymbolOwner {
            session: 0x4c50_5248,
            generation: 1,
        })
        .into_handle();
        let (tx, _rx) = async_std::channel::unbounded();
        assert!(session.borrow_mut().add_player(1, tx));
        session
    }

    fn call(session: &RuntimeSessionHandle, handler: &str, args: &[Datum]) {
        session
            .borrow_mut()
            .with_player(1, |context| {
                let arg_refs: Vec<DatumRef> = args
                    .iter()
                    .map(|datum| context.player.alloc_datum(datum.clone()))
                    .collect();
                try_call_xtra_static_handler_explicit(
                    context.player,
                    context.symbols,
                    handler,
                    &arg_refs,
                )
                .unwrap_or_else(|| panic!("{} was not dispatched to any Xtra", handler))
                .unwrap_or_else(|e| panic!("{} failed: {:?}", handler, e))
            })
            .expect("test harness player must exist");
    }

    /// `the <prop>` as a string, via the same getter the bytecode uses.
    fn movie_prop(session: &RuntimeSessionHandle, prop: &str) -> Datum {
        session
            .borrow_mut()
            .with_player(1, |context| {
                let symbol = context.symbols.intern(prop);
                let r = context.player.get_movie_prop(context.symbols, symbol)?;
                Ok::<_, ScriptError>(context.player.get_datum(&r).clone())
            })
            .expect("test harness player must exist")
            .unwrap()
    }

    fn movie_prop_string(session: &RuntimeSessionHandle, prop: &str) -> String {
        session
            .borrow_mut()
            .with_player(1, |context| {
                let symbol = context.symbols.intern(prop);
                let r = context.player.get_movie_prop(context.symbols, symbol)?;
                context.player.get_datum(&r).string_value(context.symbols)
            })
            .expect("test harness player must exist")
            .unwrap()
    }

    /// Lingo-formatted `the <prop>` — for the list-valued ones.
    fn movie_prop_formatted(session: &RuntimeSessionHandle, prop: &str) -> String {
        session
            .borrow_mut()
            .with_player(1, |context| {
                let symbol = context.symbols.intern(prop);
                let r = context.player.get_movie_prop(context.symbols, symbol)?;
                crate::player::datum_formatting::format_datum(&r, context.symbols, context.player)
            })
            .expect("test harness player must exist")
            .unwrap()
    }

    #[test]
    fn fakes_the_environment() {
        run_test(async {
            // The README's own usage example, minus the trailing `go`.
            let session = test_session();
            call(
                &session,
                "setTheMoviePath",
                &[Datum::String(
                    "http://addictinggames.com/newGames/metalmayhemworldtour/".to_string(),
                )],
            );
            call(
                &session,
                "setTheMovieName",
                &[Datum::String("metalmayhemworldtour.dcr".to_string())],
            );
            call(&session, "setTheEnvironment_shockMachine", &[Datum::Int(0)]);
            call(
                &session,
                "setThePlatform",
                &[Datum::String("Macintosh,PowerPC".to_string())],
            );
            call(
                &session,
                "setTheRunMode",
                &[Datum::String("Author".to_string())],
            );
            call(
                &session,
                "setTheEnvironment_productBuildVersion",
                &[Datum::String("593".to_string())],
            );
            call(
                &session,
                "setTheProductVersion",
                &[Datum::String("11.5".to_string())],
            );
            call(
                &session,
                "setTheEnvironment_osVersion",
                &[Datum::String("Windows,6,2,148,2,".to_string())],
            );
            call(&session, "setTheMachineType", &[Datum::Int(72)]);

            assert_eq!(
                movie_prop_string(&session, "moviePath"),
                "http://addictinggames.com/newGames/metalmayhemworldtour/"
            );
            // `the path` is the same directory string.
            assert_eq!(
                movie_prop_string(&session, "path"),
                "http://addictinggames.com/newGames/metalmayhemworldtour/"
            );
            // The name is taken verbatim, NOT derived from the path.
            assert_eq!(
                movie_prop_string(&session, "movieName"),
                "metalmayhemworldtour.dcr"
            );
            assert_eq!(
                movie_prop_string(&session, "movie"),
                "metalmayhemworldtour.dcr"
            );
            assert_eq!(movie_prop_string(&session, "platform"), "Macintosh,PowerPC");
            assert_eq!(movie_prop_string(&session, "runMode"), "Author");
            assert_eq!(movie_prop_string(&session, "productVersion"), "11.5");
            assert_eq!(movie_prop(&session, "machineType").int_value().unwrap(), 72);

            // …and the propList form agrees, including the entries that have no
            // standalone `the <prop>` accessor.
            let env = movie_prop_formatted(&session, "environmentPropList");
            assert!(env.contains("#platform: \"Macintosh,PowerPC\""), "{env}");
            assert!(env.contains("#runMode: \"Author\""), "{env}");
            assert!(env.contains("#productVersion: \"11.5\""), "{env}");
            assert!(env.contains("#productBuildVersion: \"593\""), "{env}");
            assert!(env.contains("#osVersion: \"Windows,6,2,148,2,\""), "{env}");
        });
    }

    #[test]
    fn external_params_keep_insertion_order() {
        run_test(async {
            let session = test_session();

            call(
                &session,
                "setExternalParam",
                &[
                    Datum::String("src".to_string()),
                    Datum::String("/a.dcr".to_string()),
                ],
            );
            call(
                &session,
                "setExternalParam",
                &[
                    Datum::String("sw2".to_string()),
                    Datum::String("121220".to_string()),
                ],
            );
            // An empty name is documented as invalid and must not add an entry.
            call(
                &session,
                "setExternalParam",
                &[Datum::String(String::new()), Datum::String("x".to_string())],
            );
            // Re-setting updates in place rather than appending.
            call(
                &session,
                "setExternalParam",
                &[
                    Datum::String("SRC".to_string()),
                    Datum::String("/b.dcr".to_string()),
                ],
            );

            session
                .borrow_mut()
                .with_player(1, |context| {
                    let params: Vec<(String, String)> = context
                        .player
                        .external_params
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    assert_eq!(
                        params,
                        vec![
                            ("src".to_string(), "/b.dcr".to_string()),
                            ("sw2".to_string(), "121220".to_string())
                        ]
                    );
                })
                .expect("test harness player must exist");
        });
    }

    #[test]
    fn forced_props_survive_a_movie_write() {
        run_test(async {
            let session = test_session();

            call(&session, "forceTheExitLock", &[Datum::Int(0)]);
            call(&session, "forceTheSafePlayer", &[Datum::Int(0)]);

            // A leech check re-asserting the value it wants must not stick.
            session
                .borrow_mut()
                .with_player(1, |context| {
                    context.player.set_movie_prop(
                        context.symbols,
                        Symbol::builtin(BuiltInSymbol::ExitLock),
                        Datum::Int(1),
                    )
                })
                .expect("test harness player must exist")
                .unwrap();

            assert_eq!(movie_prop(&session, "exitLock").int_value().unwrap(), 0);
            assert_eq!(movie_prop(&session, "safePlayer").int_value().unwrap(), 0);

            // And forcing it the other way reports the other way.
            call(&session, "forceTheExitLock", &[Datum::Int(1)]);
            assert_eq!(movie_prop(&session, "exitLock").int_value().unwrap(), 1);
        });
    }

    #[test]
    fn disable_flags_are_set() {
        run_test(async {
            let session = test_session();

            call(&session, "disableGoToNetMovie", &[]);
            call(&session, "disableGoToNetPage", &[]);
            // Documented no-op — must dispatch rather than raise "no handler",
            // which would abort the movie's setup script.
            call(&session, "bugfixShockwave3DBadDriverList", &[]);

            session
                .borrow_mut()
                .with_player(1, |context| {
                    assert!(context.player.env_overrides.disable_goto_net_movie);
                    assert!(context.player.env_overrides.disable_goto_net_page);
                })
                .expect("test harness player must exist");
        });
    }
}
