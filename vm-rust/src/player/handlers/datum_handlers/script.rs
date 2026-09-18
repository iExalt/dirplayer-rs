use crate::{
    director::lingo::datum::{datum_bool, Datum, DatumType},
    player::{
        allocator::ScriptInstanceAllocatorTrait,
        cast_lib::CastMemberRef,
        script::{get_lctx_for_script, ScriptInstance},
        script_ref::ScriptInstanceRef,
        symbols::{builtin::BuiltInSymbol, symbol::Symbol, symbol_table::SymbolTable},
        DatumRef, DirPlayer, ScriptError, ScriptErrorCode,
    },
};
use std::collections::VecDeque;
pub struct ScriptDatumHandlers {}

/// The synchronous portion of a script constructor. A bytecode constructor
/// is returned as an owned child invocation so the caller can suspend without
/// retaining a player borrow; virtual constructors complete in this turn.
pub(crate) enum ScriptConstructorPlan {
    Complete(DatumRef),
    Child {
        receiver: ScriptInstanceRef,
        handler_ref: crate::player::script::ScriptHandlerRef,
        args: Vec<DatumRef>,
        fallback: DatumRef,
    },
}

fn checked_datum<'a>(
    player: &'a DirPlayer,
    datum_ref: &DatumRef,
    symbols: &SymbolTable,
) -> Result<&'a Datum, ScriptError> {
    let datum = match datum_ref {
        DatumRef::Void => &Datum::Void,
        DatumRef::Ref(_) => player.allocator.try_get_datum(datum_ref).ok_or_else(|| {
            ScriptError::new_code(
                ScriptErrorCode::InvalidReference,
                format!("invalid datum reference {datum_ref}"),
            )
        })?,
    };
    if let Datum::ScriptInstanceRef(instance_ref) = datum {
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
    crate::player::compare::validate_direct_symbol_fields(datum, symbols)?;
    Ok(datum)
}

impl ScriptDatumHandlers {
    pub(crate) fn prepare_constructor(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        args: &[DatumRef],
        ctor: BuiltInSymbol,
    ) -> Result<ScriptConstructorPlan, ScriptError> {
        Self::prepare_constructor_named(player, symbols, datum, args, Symbol::builtin(ctor))
    }

    /// Prepare a named constructor, including Director's legacy `birth`
    /// constructor. The handler symbol is supplied by the owning table so the
    /// child request retains the exact symbol identity across suspension.
    pub(crate) fn prepare_constructor_named(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        args: &[DatumRef],
        ctor_symbol: Symbol,
    ) -> Result<ScriptConstructorPlan, ScriptError> {
        let script_ref = match checked_datum(player, datum, symbols)? {
            Datum::ScriptRef(script_ref) => script_ref.clone(),
            _ => {
                return Err(ScriptError::new(
                    "Cannot create new instance of non-script".to_owned(),
                ))
            }
        };
        // Constructor arguments are all consumed by either the virtual
        // implementation or the owned child request. Validate them before
        // allocating the fresh instance so a foreign handle cannot leave a
        // partially prepared constructor behind.
        for arg in args {
            checked_datum(player, arg, symbols)?;
        }
        let (instance_ref, fallback) = Self::create_script_instance(player, symbols, &script_ref)?;
        let script = player
            .movie
            .cast_manager
            .get_script_by_ref(&script_ref)
            .cloned()
            .ok_or_else(|| ScriptError::new("Script not found".to_owned()))?;
        if let Some(result) =
            crate::player::virtual_scripts::VirtualScriptRegistry::try_call_handler(
                player,
                symbols,
                &script_ref,
                Some(&instance_ref),
                ctor_symbol.clone(),
                &args.to_vec(),
            )?
        {
            let _ = result;
            return Ok(ScriptConstructorPlan::Complete(fallback));
        }
        let Some(handler_ref) = script.get_own_handler_ref(ctor_symbol) else {
            return Ok(ScriptConstructorPlan::Complete(fallback));
        };
        let expected = script
            .get_own_handler(handler_ref.1.clone())
            .map(|handler| handler.argument_name_ids.len())
            .unwrap_or(args.len());
        let mut padded_args = args.to_vec();
        padded_args.resize(expected.max(padded_args.len()), DatumRef::Void);
        Ok(ScriptConstructorPlan::Child {
            receiver: instance_ref,
            handler_ref,
            args: padded_args,
            fallback,
        })
    }

    pub(crate) fn finish_constructor(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        fallback: DatumRef,
        result: DatumRef,
    ) -> Result<DatumRef, ScriptError> {
        checked_datum(player, &result, symbols)?;
        if matches!(result, DatumRef::Void) {
            Ok(fallback)
        } else {
            Ok(result)
        }
    }
}

impl ScriptDatumHandlers {
    pub fn call(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        handler_name: Symbol,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        let handler_name_lower = symbols
            .lower(&handler_name)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        match handler_name_lower {
            "rawnew" => Self::raw_new_explicit(player, symbols, datum),
            "handler" => Self::handler(player, symbols, datum, args),
            "handlers" => Self::handlers(player, symbols, datum, args),
            // A movie script's static properties are addressable through the
            // script reference (Neopets DGS uses `script("globals")` as a global
            // data store: `g.levellist = []`, `g.levellist.add(...)`, etc.).
            // getPropRef returns the property's shared DatumRef so in-place list
            // mutation persists, mirroring the ScriptInstance handler.
            "getprop" | "getpropref" | "getaprop" => {
                let script_ref = match checked_datum(player, datum, symbols)? {
                    Datum::ScriptRef(s) => s.clone(),
                    _ => return Err(ScriptError::new("Expected script reference".to_string())),
                };
                let prop_name = checked_datum(player, &args[0], symbols)?.string_value(symbols)?;
                let prop_name = symbols.intern(&prop_name);
                let prop_ref = crate::player::script::script_get_static_prop(
                    player,
                    symbols,
                    &script_ref,
                    prop_name,
                )?;
                checked_datum(player, &prop_ref, symbols)?;
                if args.len() >= 2 {
                    // `g.prop[index]` — the bytecode passes (script, #prop, index),
                    // so index into the property value (e.g. list element). Without
                    // this the whole property was returned, ignoring the index.
                    crate::player::handlers::types::TypeUtils::get_sub_prop(
                        &prop_ref, &args[1], player, symbols,
                    )
                } else {
                    Ok(prop_ref)
                }
            }
            "setprop" | "setaprop" => {
                let script_ref = match checked_datum(player, datum, symbols)? {
                    Datum::ScriptRef(s) => s.clone(),
                    _ => return Err(ScriptError::new("Expected script reference".to_string())),
                };
                let prop_name = checked_datum(player, &args[0], symbols)?.string_value(symbols)?;
                let prop_name = symbols.intern(&prop_name);
                if args.len() >= 3 {
                    // `g.prop[index] = value`
                    let prop_ref = crate::player::script::script_get_static_prop(
                        player,
                        symbols,
                        &script_ref,
                        prop_name,
                    )?;
                    checked_datum(player, &args[2], symbols)?;
                    crate::player::handlers::types::TypeUtils::set_sub_prop(
                        &prop_ref, &args[1], &args[2], player, symbols,
                    )?;
                    Ok(args[2].clone())
                } else {
                    crate::player::script::script_set_static_prop(
                        player,
                        symbols,
                        &script_ref,
                        prop_name,
                        &args[1],
                        false,
                    )?;
                    Ok(args[1].clone())
                }
            }
            _ => Err(ScriptError::new(format!(
                "no handler {} for script datum",
                symbols.display(&handler_name).unwrap_or("<foreign symbol>")
            ))),
        }
    }

    pub fn handlers(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        _args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        let script_ref = match checked_datum(player, datum, symbols)? {
            Datum::ScriptRef(script_ref) => script_ref,
            _ => {
                return Err(ScriptError::new(
                    "Cannot get handlers of non-script".to_string(),
                ))
            }
        };
        let script = player
            .movie
            .cast_manager
            .get_script_by_ref(script_ref)
            .ok_or_else(|| ScriptError::new("Script not found".to_owned()))?;
        let handler_names = script
            .handler_names
            .iter()
            .map(|name| {
                symbols
                    .display(name)
                    .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
                Ok::<_, ScriptError>(name.clone())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let handler_name_datums: VecDeque<_> = handler_names
            .iter()
            .map(|name| player.alloc_datum(Datum::Symbol(name.clone())))
            .collect();
        Ok(player.alloc_datum(Datum::List(DatumType::List, handler_name_datums, false)))
    }

    pub fn handler(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        let name = checked_datum(player, &args[0], symbols)?.symbol_value(symbols)?;
        let script_ref = match checked_datum(player, datum, symbols)? {
            Datum::ScriptRef(script_ref) => script_ref,
            _ => {
                return Err(ScriptError::new(
                    "Cannot create new instance of non-script".to_string(),
                ))
            }
        };
        let script = player
            .movie
            .cast_manager
            .get_script_by_ref(script_ref)
            .ok_or_else(|| ScriptError::new("Script not found".to_owned()))?;
        let own_handler = script.get_own_handler(name);
        Ok(player.alloc_datum(datum_bool(own_handler.is_some())))
    }

    /// Synchronous `rawNew` preparation. The async constructor keeps its
    /// legacy wrapper until the session driver owns its continuation.
    pub fn raw_new_explicit(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        datum: &DatumRef,
    ) -> Result<DatumRef, ScriptError> {
        let script_ref = match checked_datum(player, datum, symbols)? {
            Datum::ScriptRef(script_ref) => script_ref.clone(),
            _ => {
                return Err(ScriptError::new(
                    "Cannot create new instance of non-script".to_owned(),
                ))
            }
        };
        let (_instance_ref, datum_ref) =
            Self::create_script_instance(player, symbols, &script_ref)?;
        Ok(datum_ref)
    }

    pub fn create_script_instance(
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        script_ref: &CastMemberRef,
    ) -> Result<(ScriptInstanceRef, DatumRef), ScriptError> {
        let script = player
            .movie
            .cast_manager
            .get_script_by_ref(script_ref)
            .ok_or_else(|| ScriptError::new(format!("Script not found: {:?}", script_ref)))?;
        let instance_id = player.allocator.get_free_script_instance_id();
        let lctx = get_lctx_for_script(player, script).cloned();
        let script = script.clone();
        if let Some(lctx) = lctx.as_ref() {
            let instance =
                ScriptInstance::new(instance_id, script_ref.clone(), &script, lctx, symbols);
            let instance_ref = player.allocator.alloc_script_instance(instance);
            let datum_ref = player.alloc_datum(Datum::ScriptInstanceRef(instance_ref.clone()));
            crate::player::compare::validate_direct_symbol_fields(
                player.get_datum(&datum_ref),
                symbols,
            )?;
            Ok((instance_ref, datum_ref))
        } else {
            crate::player::virtual_scripts::VirtualScriptRegistry::create_instance(
                player, symbols, script_ref,
            )
        }
    }
}
