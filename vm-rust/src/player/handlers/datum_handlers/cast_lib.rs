use crate::{
    director::lingo::datum::Datum,
    player::{
        cast_lib::CastMemberRef,
        symbols::{builtin::BuiltInSymbol, symbol::Symbol, symbol_table::SymbolTable},
        DatumRef, DirPlayer, ScriptError, ScriptErrorCode,
    },
};

pub struct CastLibDatumHandlers {}

fn checked_datum<'a>(player: &'a DirPlayer, datum: &DatumRef) -> Result<&'a Datum, ScriptError> {
    match datum {
        DatumRef::Void => Ok(&Datum::Void),
        _ => player.allocator.try_get_datum(datum).ok_or_else(|| {
            ScriptError::new_code(
                ScriptErrorCode::InvalidReference,
                format!("invalid datum reference {datum}"),
            )
        }),
    }
}

impl CastLibDatumHandlers {
    pub fn call(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        handler_name: Symbol,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        match handler_name.into_builtin() {
            Some(BuiltInSymbol::GetPropRef | BuiltInSymbol::GetProp) => {
                Self::get_prop_ref(player, symbols, datum, args)
            }
            Some(BuiltInSymbol::Count) => Self::count(player, symbols, datum, args),
            Some(BuiltInSymbol::FindEmpty) => Self::find_empty(player, datum, args),
            _ => Err(ScriptError::new_code(
                ScriptErrorCode::HandlerNotFound,
                format!(
                    "No handler {} for castLib datum",
                    symbols
                        .display(&handler_name)
                        .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
                ),
            )),
        }
    }

    fn count(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        datum: &DatumRef,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        let cast_lib_num = match checked_datum(player, datum)? {
            Datum::CastLib(num) => *num,
            _ => {
                return Err(ScriptError::new(
                    "count: datum is not a castLib".to_string(),
                ))
            }
        };

        // count(#member) returns the number of cast members
        if !args.is_empty() {
            let prop_datum = checked_datum(player, &args[0])?;
            let prop = match prop_datum.string_value(symbols) {
                Ok(value) => value,
                Err(error) if matches!(prop_datum, Datum::Symbol(_)) => return Err(error),
                Err(_) => String::new(),
            };
            if prop.eq_ignore_ascii_case("member") {
                let cast = player.movie.cast_manager.get_cast(cast_lib_num)?;
                return Ok(player.alloc_datum(Datum::Int(cast.members.len() as i32)));
            }
        }

        // Default: return member count
        let cast = player.movie.cast_manager.get_cast(cast_lib_num)?;
        Ok(player.alloc_datum(Datum::Int(cast.members.len() as i32)))
    }

    fn get_prop_ref(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        let cast_lib_num = match checked_datum(player, datum)? {
            Datum::CastLib(num) => *num,
            _ => {
                return Err(ScriptError::new(
                    "getPropRef: datum is not a castLib".to_string(),
                ))
            }
        };

        if args.is_empty() {
            return Err(ScriptError::new(
                "getPropRef requires at least one argument".to_string(),
            ));
        } else if args.len() > 2 {
            return Err(ScriptError::new(
                "getPropRef for castLib only supports one property".to_string(),
            ));
        }

        let prop_name = checked_datum(player, &args[0])?.symbol_value(symbols)?;

        match prop_name.into_builtin() {
            Some(BuiltInSymbol::Member) => {
                if args.len() < 2 {
                    return Err(ScriptError::new(
                        "getPropRef(#member, ...) requires a member name or number".to_string(),
                    ));
                }

                let member_name_or_num = checked_datum(player, &args[1])?.clone();
                let cast = player.movie.cast_manager.get_cast(cast_lib_num)?;

                let member_ref = match &member_name_or_num {
                    Datum::String(name) => {
                        cast.find_member_by_name(name).map(|member| CastMemberRef {
                            cast_lib: cast_lib_num as i32,
                            cast_member: member.number as i32,
                        })
                    }
                    Datum::Int(num) => {
                        cast.find_member_by_number(*num as u32)
                            .map(|member| CastMemberRef {
                                cast_lib: cast_lib_num as i32,
                                cast_member: member.number as i32,
                            })
                    }
                    _ => {
                        return Err(ScriptError::new(format!(
                            "getPropRef(#member, ...) expects a string or int, got {}",
                            member_name_or_num.type_str()
                        )))
                    }
                };

                match member_ref {
                    Some(mr) => Ok(player.alloc_datum(Datum::CastMember(mr))),
                    None => {
                        // Return an invalid member ref (member 0) for non-existent members
                        Ok(player.alloc_datum(Datum::CastMember(CastMemberRef {
                            cast_lib: cast_lib_num as i32,
                            cast_member: 0,
                        })))
                    }
                }
            }
            _ => Err(ScriptError::new(format!(
                "getPropRef: unknown property #{} for castLib",
                symbols
                    .display(&prop_name)
                    .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?
            ))),
        }
    }

    fn find_empty(
        player: &mut DirPlayer,
        datum: &DatumRef,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        let cast_lib_num = match checked_datum(player, datum)? {
            Datum::CastLib(num) => *num,
            _ => {
                return Err(ScriptError::new(
                    "findEmpty: datum is not a castLib".to_string(),
                ))
            }
        };

        let (c_start, c_end) = match &player.movie.file {
            Some(file) => (file.config.min_member as u32, file.config.max_member as u32),
            None => {
                return Err(ScriptError::new(
                    "findEmpty: no movie file loaded".to_string(),
                ))
            }
        };

        let start = if !args.is_empty() {
            let member_ref = checked_datum(player, &args[0])?.to_member_ref()?;
            let member_num = member_ref.cast_member as u32;
            if member_num > c_end {
                return Ok(player.alloc_datum(Datum::Int(member_num as i32)));
            }
            if member_num > c_start {
                member_num
            } else {
                c_start
            }
        } else {
            c_start
        };

        let cast = player.movie.cast_manager.get_cast(cast_lib_num)?;
        for slot in start..=c_end {
            if !cast.members.contains_key(&slot) {
                return Ok(player.alloc_datum(Datum::Int(slot as i32)));
            }
        }
        Ok(player.alloc_datum(Datum::Int(c_end as i32 + 1)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::director::lingo::datum::Datum;
    use crate::player::cast_lib::CastLib;
    use crate::player::session::RuntimeSession;
    use crate::player::symbols::symbol_table::SymbolOwner;
    use async_std::channel;

    fn test_session() -> RuntimeSession {
        let mut session = RuntimeSession::new(SymbolOwner {
            session: 81,
            generation: 1,
        });
        assert!(session.add_player(1, channel::unbounded().0));
        session
    }

    #[test]
    fn count_ignores_extra_arguments_and_get_prop_ref_returns_missing_member_ref() {
        let mut session = test_session();
        let (receiver, member_name) = session
            .with_player(1, |mut context| {
                context
                    .player
                    .movie
                    .cast_manager
                    .casts
                    .push(CastLib::test_external(1, 0));
                let receiver = context.player.alloc_datum(Datum::CastLib(1));
                let member_name = context
                    .player
                    .alloc_datum(Datum::String("missing".to_owned()));
                (receiver, member_name)
            })
            .unwrap();

        let count = session
            .with_player(1, |mut context| {
                let member_property = context
                    .player
                    .alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::Member)));
                let mut foreign = test_session();
                let ignored_foreign_argument = foreign
                    .with_player(1, |mut foreign_context| {
                        foreign_context
                            .player
                            .alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::Member)))
                    })
                    .unwrap();
                let args = vec![member_property, ignored_foreign_argument];
                let result = CastLibDatumHandlers::call(
                    context.player,
                    context.symbols,
                    &receiver,
                    Symbol::builtin(BuiltInSymbol::Count),
                    &args,
                )?;
                Ok::<_, ScriptError>(context.player.get_datum(&result).clone())
            })
            .unwrap()
            .unwrap();
        assert!(matches!(count, Datum::Int(0)));

        let missing = session
            .with_player(1, |mut context| {
                let property = context
                    .player
                    .alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::Member)));
                let args = vec![property, member_name.clone()];
                let result = CastLibDatumHandlers::call(
                    context.player,
                    context.symbols,
                    &receiver,
                    Symbol::builtin(BuiltInSymbol::GetPropRef),
                    &args,
                )?;
                Ok::<_, ScriptError>(context.player.get_datum(&result).clone())
            })
            .unwrap()
            .unwrap();
        assert!(matches!(
            missing,
            Datum::CastMember(CastMemberRef {
                cast_lib: 1,
                cast_member: 0,
            })
        ));
    }

    #[test]
    fn foreign_receiver_and_consumed_argument_are_typed_invalid_references() {
        let mut foreign = test_session();
        let foreign_receiver = foreign
            .with_player(1, |mut context| {
                context.player.alloc_datum(Datum::CastLib(1))
            })
            .unwrap();
        let mut local = test_session();
        local
            .with_player(1, |mut context| {
                context
                    .player
                    .movie
                    .cast_manager
                    .casts
                    .push(CastLib::test_external(1, 0));
                let error = CastLibDatumHandlers::call(
                    context.player,
                    context.symbols,
                    &foreign_receiver,
                    Symbol::builtin(BuiltInSymbol::Count),
                    &Vec::new(),
                )
                .unwrap_err();
                assert_eq!(error.code, ScriptErrorCode::InvalidReference);

                let receiver = context.player.alloc_datum(Datum::CastLib(1));
                let foreign_arg = foreign
                    .with_player(1, |mut foreign_context| {
                        foreign_context
                            .player
                            .alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::Member)))
                    })
                    .unwrap();
                let error = CastLibDatumHandlers::call(
                    context.player,
                    context.symbols,
                    &receiver,
                    Symbol::builtin(BuiltInSymbol::Count),
                    &vec![foreign_arg],
                )
                .unwrap_err();
                assert_eq!(error.code, ScriptErrorCode::InvalidReference);

                let foreign_symbol = foreign
                    .with_player(1, |mut foreign_context| {
                        foreign_context.symbols.intern("foreignCountProperty")
                    })
                    .unwrap();
                let local_foreign_symbol =
                    context.player.alloc_datum(Datum::Symbol(foreign_symbol));
                let error = CastLibDatumHandlers::call(
                    context.player,
                    context.symbols,
                    &receiver,
                    Symbol::builtin(BuiltInSymbol::Count),
                    &vec![local_foreign_symbol],
                )
                .unwrap_err();
                assert_eq!(error.code, ScriptErrorCode::InvalidReference);
            })
            .unwrap();
    }
}
