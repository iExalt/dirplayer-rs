use crate::{
    director::lingo::datum::Datum,
    player::{
        allocator::ScriptInstanceAllocatorTrait,
        reserve_player_mut, reserve_player_ref,
        symbols::{builtin::BuiltInSymbol, symbol::Symbol, symbol_table::SymbolTable},
        timeout::Timeout,
        DatumRef, DirPlayer, ScriptError, ScriptErrorCode,
    },
};

pub struct TimeoutDatumHandlers {}

pub(crate) enum TimeoutNewPlan {
    Complete(DatumRef),
    Child {
        receiver: crate::player::script_ref::ScriptInstanceRef,
        handler_ref: crate::player::script::ScriptHandlerRef,
        args: Vec<DatumRef>,
        fallback: DatumRef,
        timeout_name: String,
    },
}

pub(crate) enum TimeoutForgetPlan {
    Complete(DatumRef),
    Child {
        receiver: crate::player::script_ref::ScriptInstanceRef,
        handler_ref: crate::player::script::ScriptHandlerRef,
        timeout_name: String,
    },
}

fn validate_timeout_target(
    player: &DirPlayer,
    symbols: &SymbolTable,
    target: &DatumRef,
) -> Result<DatumRef, ScriptError> {
    let target_datum = match target {
        DatumRef::Void => &Datum::Void,
        _ => player.allocator.try_get_datum(target).ok_or_else(|| {
            ScriptError::new_code(
                ScriptErrorCode::InvalidReference,
                format!("invalid datum reference {target}"),
            )
        })?,
    };
    crate::player::compare::validate_direct_symbol_fields(target_datum, symbols)?;
    if let Datum::ScriptInstanceRef(instance_ref) = target_datum {
        player
            .allocator
            .get_script_instance_opt(instance_ref)
            .ok_or_else(|| {
                ScriptError::new_code(
                    ScriptErrorCode::InvalidReference,
                    "foreign or stale ScriptInstanceRef timeout target".to_owned(),
                )
            })?;
    }
    Ok(target.clone())
}

fn checked_datum<'a>(
    player: &'a DirPlayer,
    datum: &DatumRef,
    symbols: &SymbolTable,
) -> Result<&'a Datum, ScriptError> {
    let value = match datum {
        DatumRef::Void => &Datum::Void,
        _ => player.allocator.try_get_datum(datum).ok_or_else(|| {
            ScriptError::new_code(
                ScriptErrorCode::InvalidReference,
                format!("invalid datum reference {datum}"),
            )
        })?,
    };
    crate::player::compare::validate_direct_symbol_fields(value, symbols)?;
    if let Datum::ScriptInstanceRef(instance_ref) = value {
        player
            .allocator
            .get_script_instance_opt(instance_ref)
            .ok_or_else(|| {
                ScriptError::new_code(
                    ScriptErrorCode::InvalidReference,
                    "foreign or stale ScriptInstanceRef".to_owned(),
                )
            })?;
    }
    Ok(value)
}

impl TimeoutDatumHandlers {
    #[allow(dead_code, unused_variables)]
    pub fn call(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        handler_name: Symbol,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        symbols
            .lower(&handler_name)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        match handler_name.into_builtin() {
            Some(BuiltInSymbol::Forget) => Self::forget(player, symbols, datum, args),
            Some(BuiltInSymbol::SetAt) => Self::set_at(player, symbols, datum, args),
            _ => Err(ScriptError::new(format!(
                "No handler {} for timeout",
                symbols.display(&handler_name).unwrap_or("<foreign symbol>")
            ))),
        }
    }

    fn set_at(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        // TimeoutInstance needs to support setAt for #ancestor to work with Object Manager
        // We silently ignore ancestor setting since timeouts don't use ancestor chains
        let key = checked_datum(player, &args[0], symbols)?.symbol_value(symbols)?;
        match key.into_builtin() {
            Some(BuiltInSymbol::Ancestor) => {
                // Silently accept but ignore - timeouts don't use ancestor chains.
                // Do not inspect value or extra arguments on this no-op path.
                Ok(DatumRef::Void)
            }
            _ => Err(ScriptError::new(format!(
                "Cannot setAt property {} on timeout",
                symbols.display(&key).unwrap_or("<foreign symbol>")
            ))),
        }
    }

    /// Prepare timeout construction while the session owns the player. Script
    /// backed timeouts return an owned child invocation; the wrapper is created
    /// only after that child completes.
    pub fn prepare_new(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        args: &[DatumRef],
    ) -> Result<TimeoutNewPlan, ScriptError> {
        let timeout_datum = checked_datum(player, datum, symbols)?;
        let timeout_name = match timeout_datum {
            Datum::TimeoutFactory => {
                let name = args.first().ok_or_else(|| {
                    ScriptError::new("timeout.new() requires at least a name argument".to_owned())
                })?;
                checked_datum(player, name, symbols)?.string_value(symbols)?
            }
            Datum::TimeoutRef(timeout_name) => timeout_name.clone(),
            _ => {
                return Err(ScriptError::new(
                    "Cannot create timeout from non-timeout".to_owned(),
                ));
            }
        };
        let (period_arg, handler_arg, target_arg) = match timeout_datum {
            Datum::TimeoutFactory => {
                if args.len() < 3 {
                    return Err(ScriptError::new(
                        "timeout.new() requires at least: name, period, handler".to_owned(),
                    ));
                }
                (1, 2, args.get(3).map(|_| 3))
            }
            Datum::TimeoutRef(_) => {
                if args.len() < 2 {
                    return Err(ScriptError::new(
                        "timeout(name).new() requires at least: period, handler".to_owned(),
                    ));
                }
                (0, 1, args.get(2).map(|_| 2))
            }
            _ => unreachable!(),
        };

        if player.movie.dir_version >= 1000 {
            if let Some(script_ref) = player
                .movie
                .cast_manager
                .find_member_ref_by_name(&timeout_name)
            {
                if player
                    .movie
                    .cast_manager
                    .get_script_by_ref(&script_ref)
                    .is_some()
                {
                    use super::script::ScriptDatumHandlers;
                    let script_datum = player.alloc_datum(Datum::ScriptRef(script_ref));
                    match ScriptDatumHandlers::prepare_constructor(
                        player,
                        symbols,
                        &script_datum,
                        args,
                        BuiltInSymbol::New,
                    )? {
                        super::script::ScriptConstructorPlan::Complete(instance) => {
                            return Ok(TimeoutNewPlan::Complete(player.alloc_datum(
                                Datum::timeout_instance(
                                    timeout_name,
                                    0,
                                    DatumRef::Void,
                                    DatumRef::Void,
                                    Some(instance),
                                ),
                            )));
                        }
                        super::script::ScriptConstructorPlan::Child {
                            receiver,
                            handler_ref,
                            args,
                            fallback,
                        } => {
                            return Ok(TimeoutNewPlan::Child {
                                receiver,
                                handler_ref,
                                args,
                                fallback,
                                timeout_name,
                            });
                        }
                    }
                }
            }
        }

        let timeout_period = checked_datum(player, &args[period_arg], symbols)?.int_value()?;
        let timeout_handler = match checked_datum(player, &args[handler_arg], symbols)? {
            Datum::String(value) => symbols.intern(value),
            Datum::Symbol(value) => value.clone(),
            _ => {
                return Err(ScriptError::new(
                    "Timeout handler must be a string or symbol".to_owned(),
                ));
            }
        };
        let target_ref = target_arg
            .map(|index| args[index].clone())
            .unwrap_or(DatumRef::Void);
        checked_datum(player, &target_ref, symbols)?;
        let timeout_period = timeout_period.max(0) as u32;

        let timeout = Timeout {
            handler: timeout_handler,
            name: timeout_name.clone(),
            period: timeout_period,
            target_ref: target_ref.clone(),
            is_scheduled: false,
            incarnation: 0,
            next_fire_ms: 0.0,
        };

        player.replace_timeout(timeout)?;
        Ok(TimeoutNewPlan::Complete(player.alloc_datum(
            Datum::timeout_instance(
                timeout_name,
                timeout_period as i32,
                args[handler_arg].clone(),
                target_ref,
                None,
            ),
        )))
    }

    pub fn finish_new(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        timeout_name: String,
        fallback: DatumRef,
        result: DatumRef,
    ) -> Result<DatumRef, ScriptError> {
        let instance = super::script::ScriptDatumHandlers::finish_constructor(
            player, symbols, fallback, result,
        )?;
        Ok(player.alloc_datum(Datum::timeout_instance(
            timeout_name,
            0,
            DatumRef::Void,
            DatumRef::Void,
            Some(instance),
        )))
    }

    pub fn has_async_handler(name: Symbol) -> bool {
        matches!(name.into_builtin(), Some(BuiltInSymbol::New))
    }

    pub fn prepare_forget(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
    ) -> Result<TimeoutForgetPlan, ScriptError> {
        let timeout_name = match checked_datum(player, datum, symbols)? {
            Datum::TimeoutRef(name) => name.clone(),
            Datum::TimeoutInstance(instance) => instance.name.clone(),
            _ => return Err(ScriptError::new("Cannot forget non-timeout".to_owned())),
        };
        let script_instance = match checked_datum(player, datum, symbols)? {
            Datum::TimeoutInstance(instance) => instance.script_instance.clone(),
            _ => None,
        };
        if let Some(instance) = script_instance {
            if let Some(handler_ref) = super::script_instance::ScriptInstanceUtils::get_handler(
                Symbol::builtin(BuiltInSymbol::Destroy),
                &instance,
                player,
            )? {
                let receiver = match checked_datum(player, &instance, symbols)? {
                    Datum::ScriptInstanceRef(receiver) => receiver.clone(),
                    _ => {
                        return Err(ScriptError::new_code(
                            ScriptErrorCode::InvalidReference,
                            "timeout script instance is stale".to_owned(),
                        ));
                    }
                };
                return Ok(TimeoutForgetPlan::Child {
                    receiver,
                    handler_ref,
                    timeout_name,
                });
            }
        }
        player.forget_timeout(&timeout_name);
        Ok(TimeoutForgetPlan::Complete(DatumRef::Void))
    }

    pub fn finish_forget(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        timeout_name: String,
        result: DatumRef,
    ) -> Result<DatumRef, ScriptError> {
        checked_datum(player, &result, symbols)?;
        player.forget_timeout(&timeout_name);
        Ok(DatumRef::Void)
    }

    pub fn has_forget_async_handler(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
    ) -> Result<bool, ScriptError> {
        let timeout = checked_datum(player, datum, symbols)?;
        Ok(
            matches!(timeout, Datum::TimeoutInstance(instance) if instance.script_instance.is_some()),
        )
    }

    fn forget(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        _: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        let timeout_name = match checked_datum(player, datum, symbols)? {
            Datum::TimeoutRef(timeout_name) => timeout_name.to_owned(),
            Datum::TimeoutInstance(ti) => ti.name.to_owned(),
            _ => return Err(ScriptError::new("Cannot forget non-timeout".to_string())),
        };
        player.forget_timeout(&timeout_name);
        Ok(DatumRef::Void)
    }

    pub fn get_prop(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        prop: Symbol,
    ) -> Result<DatumRef, ScriptError> {
        let timeout_datum = checked_datum(player, datum, symbols)?;
        let prop_name = symbols
            .display(&prop)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        let prop_lower = symbols
            .lower(&prop)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        match timeout_datum {
            Datum::TimeoutRef(timeout_name) => {
                let timeout = player.timeout_manager.get_timeout(timeout_name);
                match prop_lower {
                    "name" => Ok(player.alloc_datum(Datum::String(timeout_name.to_owned()))),
                    "target" => {
                        let target = timeout.map_or(DatumRef::Void, |x| x.target_ref.clone());
                        validate_timeout_target(player, symbols, &target)
                    }
                    "period" => {
                        let p = timeout.map_or(0, |t| t.period as i32);
                        Ok(player.alloc_datum(Datum::Int(p)))
                    }
                    _ => Err(ScriptError::new(format!(
                        "Cannot get timeout property {}",
                        prop_name
                    ))),
                }
            }
            Datum::TimeoutInstance(ti) => match prop_lower {
                "name" => Ok(player.alloc_datum(Datum::String(ti.name.to_owned()))),
                "target" => validate_timeout_target(player, symbols, &ti.target),
                "period" => {
                    let p = player
                        .timeout_manager
                        .get_timeout(&ti.name)
                        .map_or(0, |t| t.period as i32);
                    Ok(player.alloc_datum(Datum::Int(p)))
                }
                _ => Err(ScriptError::new(format!(
                    "Cannot get timeout property {}",
                    prop_name
                ))),
            },
            _ => Err(ScriptError::new(
                "Cannot get prop of non-timeout".to_string(),
            )),
        }
    }

    pub fn set_prop(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        prop: Symbol,
        value: &DatumRef,
    ) -> Result<(), ScriptError> {
        let timeout_datum = checked_datum(player, datum, symbols)?;
        let timeout_name = match timeout_datum {
            Datum::TimeoutRef(timeout_name) => timeout_name.clone(),
            Datum::TimeoutInstance(ti) => ti.name.clone(),
            _ => {
                return Err(ScriptError::new(
                    "Cannot set prop of non-timeout".to_string(),
                ));
            }
        };

        symbols
            .lower(&prop)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        match prop.into_builtin() {
            Some(BuiltInSymbol::Target) => {
                if player.timeout_manager.get_timeout(&timeout_name).is_none() {
                    return Err(ScriptError::new(
                        "Cannot set target of unscheduled timeout".to_string(),
                    ));
                }
                let new_target = validate_timeout_target(player, symbols, value)?;
                let timeout = player.timeout_manager.get_timeout_mut(&timeout_name);
                if let Some(timeout) = timeout {
                    timeout.target_ref = new_target;
                    Ok(())
                } else {
                    Err(ScriptError::new(
                        "Cannot set target of unscheduled timeout".to_string(),
                    ))
                }
            }
            // `the period of timeoutObject` is read/write (Director Scripting
            // Dictionary): it is the number of ms between timeout events, and
            // setting it changes the interval. We update the period and
            // reschedule the underlying timer (a period of 0 makes it dormant).
            // Habbo's DM_CurlDetacher sets `period = 1` to start a dormant
            // timeout — that drives the cURL-download completion callback. Before
            // this arm existed, the assignment errored ("Cannot set timeout
            // property period") and, because it runs inside the cURL Xtra
            // callback (which fails silently), the download never completed and
            // ES Origins never showed its login (figuredata/external_texts
            // never loaded).
            Some(BuiltInSymbol::Period) => {
                let new_period = checked_datum(player, value, symbols)?.int_value()?;
                let new_period = if new_period < 0 { 0 } else { new_period as u32 };
                if player.set_timeout_period(&timeout_name, new_period)? {
                    Ok(())
                } else {
                    Err(ScriptError::new(
                        "Cannot set period of unscheduled timeout".to_string(),
                    ))
                }
            }
            _ => Err(ScriptError::new(format!(
                "Cannot set timeout property {}",
                symbols.display(&prop).unwrap_or("<foreign symbol>")
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::cast_lib::CastMemberRef;
    use crate::player::ownership::OwnerToken;
    use crate::player::script::ScriptInstance;
    use async_std::channel;

    fn test_player() -> DirPlayer {
        let (tx, _rx) = channel::unbounded();
        DirPlayer::new_with_owner(tx, OwnerToken::transitional())
    }

    #[test]
    fn set_at_ancestor_ignores_receiver_value_and_extra_arguments() {
        let mut player = test_player();
        let mut foreign_player = test_player();
        let mut symbols = SymbolTable::new();
        let key = player.alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::Ancestor)));
        let foreign_receiver = foreign_player.alloc_datum(Datum::Int(1));
        let foreign_value = foreign_player.alloc_datum(Datum::Int(7));
        let foreign_extra = foreign_player.alloc_datum(Datum::Int(9));

        let result = TimeoutDatumHandlers::set_at(
            &mut player,
            &mut symbols,
            &foreign_receiver,
            &vec![key, foreign_value, foreign_extra],
        );
        assert!(result.is_ok());
    }

    #[test]
    fn unscheduled_timeout_target_ignores_foreign_value() {
        let mut player = test_player();
        let mut foreign_player = test_player();
        let symbols = SymbolTable::new();
        let timeout_ref = player.alloc_datum(Datum::TimeoutRef("unscheduled-target".to_owned()));
        let foreign_value = foreign_player.alloc_datum(Datum::Int(4));

        let error = TimeoutDatumHandlers::set_prop(
            &mut player,
            &symbols,
            &timeout_ref,
            Symbol::builtin(BuiltInSymbol::Target),
            &foreign_value,
        )
        .expect_err("unscheduled target must fail before inspecting its value");
        assert_ne!(error.code, ScriptErrorCode::InvalidReference);
        assert!(error.message.contains("unscheduled"));
    }

    #[test]
    fn scheduled_timeout_rejects_foreign_target_before_mutation() {
        let mut player = test_player();
        let mut foreign_player = test_player();
        let mut symbols = SymbolTable::new();
        let name = "scheduled-target".to_owned();
        let timeout_ref = player.alloc_datum(Datum::TimeoutRef(name.clone()));
        player
            .replace_timeout(Timeout {
                name: name.clone(),
                period: 100,
                handler: Symbol::builtin(BuiltInSymbol::Forget),
                target_ref: DatumRef::Void,
                is_scheduled: false,
                incarnation: 0,
                next_fire_ms: 0.0,
            })
            .expect("test timeout replacement must succeed");
        let foreign_instance = foreign_player
            .allocator
            .alloc_script_instance(ScriptInstance {
                instance_id: 1,
                script: CastMemberRef {
                    cast_lib: 1,
                    cast_member: 1,
                },
                ancestor: None,
                properties: Default::default(),
                begin_sprite_called: false,
            });
        let foreign_target = player.alloc_datum(Datum::ScriptInstanceRef(foreign_instance));

        let error = TimeoutDatumHandlers::set_prop(
            &mut player,
            &symbols,
            &timeout_ref,
            Symbol::builtin(BuiltInSymbol::Target),
            &foreign_target,
        )
        .expect_err("foreign target must be rejected");
        assert_eq!(error.code, ScriptErrorCode::InvalidReference);
        assert!(matches!(
            player
                .timeout_manager
                .get_timeout(&name)
                .unwrap()
                .target_ref,
            DatumRef::Void
        ));
    }
}
