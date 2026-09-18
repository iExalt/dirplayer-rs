pub mod javascript_proxy;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use fxhash::FxHashMap;

use crate::director::chunks::script::ScriptChunk;
use crate::director::enums::ScriptType;
use crate::director::lingo::datum::Datum;
use crate::player::symbols::symbol::Symbol;

use super::allocator::ScriptInstanceAllocatorTrait;
use super::cast_lib::{cast_member_ref, CastMemberRef};
use super::cast_member::{CastMember, CastMemberType, ScriptMember};
use super::ci_string::CiString;
use super::script::{Script, ScriptInstance};
use super::script_ref::ScriptInstanceRef;
use super::sprite::ColorRef;
use super::symbols::{symbol::SymbolError, symbol_table::SymbolTable};
use super::{DatumRef, DirPlayer, ScriptError};

fn validate_symbol(symbols: &SymbolTable, name: &Symbol) -> Result<(), ScriptError> {
    symbols
        .lower(name)
        .map(|_| ())
        .map_err(|_| ScriptError::from(SymbolError::Foreign))
}

fn validate_instance_owner(
    player: &DirPlayer,
    instance_ref: &ScriptInstanceRef,
) -> Result<(), ScriptError> {
    let owner = player.allocator.owner_token();
    if !instance_ref.owner().same_identity(&owner) || !owner.is_arena_live() {
        return Err(ScriptError::new(
            "foreign or stale ScriptInstanceRef".to_owned(),
        ));
    }
    Ok(())
}

fn validate_live_instance(
    player: &DirPlayer,
    instance_ref: &ScriptInstanceRef,
) -> Result<(), ScriptError> {
    validate_instance_owner(player, instance_ref)?;
    if player
        .allocator
        .get_script_instance_opt(instance_ref)
        .is_none()
    {
        return Err(ScriptError::new(
            "foreign or stale ScriptInstanceRef".to_owned(),
        ));
    }
    Ok(())
}

/// Trait that Rust code implements to provide a virtual script's behavior.
///
/// Virtual scripts can either be entirely new scripts or partial overrides
/// of existing bytecode scripts. All methods return `Option` to support
/// partial overrides — returning `None` falls through to the standard
/// bytecode implementation.
pub trait VirtualScriptHandler {
    /// The Director script type for this virtual script.
    /// Determines how the script is registered in the cast and whether its
    /// handlers are callable globally (Movie) or only on instances (Parent).
    fn script_type(&self) -> ScriptType {
        ScriptType::Parent
    }

    /// Check if this virtual script handles the given handler name.
    /// Used by `has_async_handler` checks to avoid claiming support for
    /// handler names the virtual script doesn't actually implement.
    fn has_handler(&self, symbols: &SymbolTable, name: Symbol) -> Result<bool, ScriptError> {
        validate_symbol(symbols, &name)?;
        Ok(true)
    }

    /// Property names for new virtual scripts (used when creating instances without lctx).
    fn get_property_names(&self) -> Vec<Symbol> {
        vec![]
    }

    /// Try to handle a handler call. Return `Ok(Some(result))` to handle,
    /// `Ok(None)` to fall through to bytecode. `Err` to propagate an error.
    ///
    /// `instance` is `None` for movie script calls with no receiver instance.
    fn call_handler(
        &self,
        _player: &mut DirPlayer,
        symbols: &SymbolTable,
        _instance: Option<&ScriptInstanceRef>,
        name: Symbol,
        _args: &Vec<DatumRef>,
    ) -> Result<Option<DatumRef>, ScriptError> {
        validate_symbol(symbols, &name)?;
        Ok(None)
    }

    /// Try to get a property. `Ok(Some(datum))` = handled, `Ok(None)` = fall through.
    fn get_prop(
        &self,
        _player: &mut DirPlayer,
        symbols: &SymbolTable,
        _instance: &ScriptInstanceRef,
        name: Symbol,
    ) -> Result<Option<DatumRef>, ScriptError> {
        validate_symbol(symbols, &name)?;
        Ok(None)
    }

    /// Try to set a property. `Ok(Some(()))` = handled, `Ok(None)` = fall through.
    fn set_prop(
        &self,
        _player: &mut DirPlayer,
        symbols: &SymbolTable,
        _instance: &ScriptInstanceRef,
        name: Symbol,
        _value: &DatumRef,
    ) -> Result<Option<()>, ScriptError> {
        validate_symbol(symbols, &name)?;
        Ok(None)
    }
}

/// Manages registration, instance creation, and dispatch for virtual scripts.
///
/// Virtual scripts are Rust-implemented scripts injected into the movie's cast,
/// allowing the player to intercept handler calls, property access, and instance
/// creation without requiring Director bytecode.
pub struct VirtualScriptRegistry;

impl VirtualScriptRegistry {
    // -----------------------------------------------------------------------
    // Registration
    // -----------------------------------------------------------------------

    /// Register a completely new virtual script. Creates a Script+CastMember
    /// in cast_lib 1 (the internal cast) and stores the handler.
    ///
    /// Returns the `CastMemberRef` for the newly created script.
    pub fn register(
        player: &mut DirPlayer,
        name: &str,
        handler: Rc<dyn VirtualScriptHandler>,
    ) -> CastMemberRef {
        let script_type = handler.script_type();
        // Key the script OUTSIDE the cast-member number space, and don't give
        // it a CastMember at all — a virtual script is player machinery and
        // must be invisible to the movie's own view of its cast.
        //
        // Both Nabisco World mini-golf titles audit their entire cast against a
        // `castmems` list of expected member types before they will show a
        // hole's menu thumbnail, and `the type of member N` reports #empty for
        // an unused slot. Occupying ANY slot in the audited range breaks that:
        // filling the first interior gap put a #script at member 26 of Mini
        // Mini-Golf (whose cast fills 1..25), and appending after the last
        // authored slot merely moved the problem into Mini-Golf's 523..528,
        // since its hole 1 spans 1..528 while its cast ends at 522.
        //
        // 1000000+ is the offset this file's sibling already uses for the same
        // reason (`get_behavior_script_from_lctx`: "avoid collision with cast
        // member numbers"). `get_script_for_member` looks in `scripts` before
        // `members`, so resolution is unaffected, and `the number of
        // castMembers` (max member id) no longer moves.
        let member_number = 2_000_000 + player.virtual_scripts.len() as u32;
        let cast = &mut player.movie.cast_manager.casts[0]; // cast_lib 1
        let member_ref = cast_member_ref(cast.number as i32, member_number as i32);

        // Create a stub Script with no bytecode
        let script = Script {
            member_ref: member_ref.clone(),
            name: name.to_string(),
            chunk: ScriptChunk {
                script_number: 0,
                literals: vec![],
                handlers: vec![],
                property_name_ids: vec![],
                property_defaults: HashMap::new(),
            },
            script_type,
            handlers: FxHashMap::default(),
            handler_names: vec![],
            handler_names_raw: vec![],
            properties: RefCell::new(FxHashMap::default()),
        };

        // Insert directly (bypass insert_member which requires lctx for
        // scripts). No CastMember: see the note on `member_number` above —
        // `find_by_name` resolves virtual scripts instead.
        cast.scripts.insert(member_number, Rc::new(script));

        // Store the virtual handler
        player.virtual_scripts.insert(member_ref.clone(), handler);

        // Invalidate movie script cache
        player.movie.cast_manager.clear_movie_script_cache();

        member_ref
    }

    /// Resolve a registered virtual script by name.
    ///
    /// Virtual scripts deliberately have no CastMember (see `register`), so
    /// `find_member_ref_by_name` can't see them — look them up through the
    /// registry instead.
    pub fn find_by_name(player: &DirPlayer, name: &str) -> Option<CastMemberRef> {
        player
            .virtual_scripts
            .keys()
            .find(|member_ref| {
                player
                    .movie
                    .cast_manager
                    .get_script_by_ref(member_ref)
                    .map_or(false, |script| script.name.eq_ignore_ascii_case(name))
            })
            .cloned()
    }

    /// Attach a virtual handler to an existing script for partial overrides.
    ///
    /// The virtual handler's methods will be called first; returning `None`
    /// falls through to the script's original bytecode implementation.
    pub fn attach(
        player: &mut DirPlayer,
        script_member_ref: CastMemberRef,
        handler: Rc<dyn VirtualScriptHandler>,
    ) {
        player.virtual_scripts.insert(script_member_ref, handler);
    }

    // -----------------------------------------------------------------------
    // Instance creation
    // -----------------------------------------------------------------------

    /// Create a ScriptInstance for a virtual script (one without lctx/bytecode).
    ///
    /// Populates properties from `VirtualScriptHandler::get_property_names()`,
    /// allocates the instance, and returns the refs.
    pub fn create_instance(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        script_ref: &CastMemberRef,
    ) -> Result<(ScriptInstanceRef, DatumRef), ScriptError> {
        let instance_id = player.allocator.get_free_script_instance_id();
        let mut properties = FxHashMap::default();
        if let Some(vh) = player.virtual_scripts.get(script_ref) {
            for prop_name in vh.get_property_names() {
                validate_symbol(symbols, &prop_name)?;
                properties.insert(prop_name, DatumRef::Void);
            }
        }
        let instance = ScriptInstance {
            instance_id,
            script: script_ref.to_owned(),
            ancestor: None,
            properties,
            begin_sprite_called: false,
        };
        let instance_ref = player.allocator.alloc_script_instance(instance);
        let datum_ref = player.alloc_datum(Datum::ScriptInstanceRef(instance_ref.clone()));
        Ok((instance_ref, datum_ref))
    }

    // -----------------------------------------------------------------------
    // Handler existence checks
    // -----------------------------------------------------------------------

    /// Check if a virtual handler is registered for the given script and handler name.
    pub fn has_script_handler(
        player: &DirPlayer,
        symbols: &SymbolTable,
        script_ref: &CastMemberRef,
        name: Symbol,
    ) -> Result<bool, ScriptError> {
        validate_symbol(symbols, &name)?;
        player
            .virtual_scripts
            .get(script_ref)
            .map_or(Ok(false), |vh| vh.has_handler(symbols, name))
    }

    /// Check if a virtual handler is registered for the given instance's script
    /// and handler name.
    pub fn has_instance_handler(
        player: &DirPlayer,
        symbols: &SymbolTable,
        instance_ref: &ScriptInstanceRef,
        name: Symbol,
    ) -> Result<bool, ScriptError> {
        validate_symbol(symbols, &name)?;
        if validate_instance_owner(player, instance_ref).is_err() {
            return Ok(false);
        }
        let Some(instance) = player.allocator.get_script_instance_opt(instance_ref) else {
            return Ok(false);
        };
        Self::has_script_handler(player, symbols, &instance.script, name)
    }

    // -----------------------------------------------------------------------
    // Dispatch helpers
    // -----------------------------------------------------------------------

    /// Try to dispatch a handler call to a virtual script by CastMemberRef.
    /// Returns `Ok(Some(result))` if handled, `Ok(None)` to fall through,
    /// `Err` to propagate.
    pub fn try_call_handler(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        script_ref: &CastMemberRef,
        instance: Option<&ScriptInstanceRef>,
        name: Symbol,
        args: &Vec<DatumRef>,
    ) -> Result<Option<DatumRef>, ScriptError> {
        validate_symbol(symbols, &name)?;
        if let Some(instance_ref) = instance {
            validate_live_instance(player, instance_ref)?;
        }
        if let Some(vh) = player.virtual_scripts.get(script_ref).cloned() {
            vh.call_handler(player, symbols, instance, name, args)
        } else {
            Ok(None)
        }
    }

    /// Try to dispatch a handler call to a virtual script via an instance's
    /// script ref.
    pub fn try_call_instance_handler(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        instance_ref: &ScriptInstanceRef,
        name: Symbol,
        args: &Vec<DatumRef>,
    ) -> Result<Option<DatumRef>, ScriptError> {
        validate_symbol(symbols, &name)?;
        validate_live_instance(player, instance_ref)?;
        let script_ref = player
            .allocator
            .get_script_instance_opt(instance_ref)
            .ok_or_else(|| ScriptError::new("foreign or stale ScriptInstanceRef".to_owned()))?
            .script
            .clone();
        Self::try_call_handler(player, symbols, &script_ref, Some(instance_ref), name, args)
    }

    /// Try to get a property from a virtual script handler.
    pub fn try_get_instance_prop(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        instance_ref: &ScriptInstanceRef,
        name: Symbol,
    ) -> Result<Option<DatumRef>, ScriptError> {
        validate_symbol(symbols, &name)?;
        validate_instance_owner(player, instance_ref)?;
        // Bail before touching the arena. This runs ahead of the ordinary
        // instance-property lookup on EVERY `getprop`, and most movies register
        // no virtual scripts at all — so the common path was paying an arena
        // lookup, a CastMemberRef clone and a hash lookup to learn nothing.
        // `getprop` is the single hottest opcode in Agent Free Ride (14.9%).
        if player.virtual_scripts.is_empty() {
            return Ok(None);
        }
        let script_ref = player
            .allocator
            .get_script_instance_opt(instance_ref)
            .ok_or_else(|| ScriptError::new("foreign or stale ScriptInstanceRef".to_owned()))?
            .script
            .clone();
        if let Some(vh) = player.virtual_scripts.get(&script_ref).cloned() {
            vh.get_prop(player, symbols, instance_ref, name)
        } else {
            Ok(None)
        }
    }

    /// Try to set a property via a virtual script handler.
    pub fn try_set_instance_prop(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        instance_ref: &ScriptInstanceRef,
        name: Symbol,
        value: &DatumRef,
    ) -> Result<Option<()>, ScriptError> {
        validate_symbol(symbols, &name)?;
        validate_instance_owner(player, instance_ref)?;
        // See `try_get_instance_prop`: skip the arena + clone + hash when no
        // virtual script is registered.
        if player.virtual_scripts.is_empty() {
            return Ok(None);
        }
        let script_ref = player
            .allocator
            .get_script_instance_opt(instance_ref)
            .ok_or_else(|| ScriptError::new("foreign or stale ScriptInstanceRef".to_owned()))?
            .script
            .clone();
        if let Some(vh) = player.virtual_scripts.get(&script_ref).cloned() {
            vh.set_prop(player, symbols, instance_ref, name, value)
        } else {
            Ok(None)
        }
    }

    /// Try to dispatch a handler call to any registered virtual movie script
    /// (for global handler calls). Only virtual scripts with `ScriptType::Movie`
    /// are eligible, matching Director's semantics.
    pub fn try_call_any_global_handler(
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        name: Symbol,
        args: &Vec<DatumRef>,
    ) -> Result<Option<DatumRef>, ScriptError> {
        validate_symbol(symbols, &name)?;
        let handlers: Vec<_> = player.virtual_scripts.values().cloned().collect();
        for vh in handlers {
            if vh.script_type() != ScriptType::Movie {
                continue;
            }
            if let Some(result) = vh.call_handler(player, symbols, None, name.clone(), args)? {
                return Ok(Some(result));
            }
        }
        Ok(None)
    }
}

/// Register all built-in virtual scripts.
pub fn register_virtual_scripts(player: &mut DirPlayer) {
    // VirtualScriptRegistry::register(player, "JavaScriptProxy", Rc::new(javascript_proxy::JavascriptProxy));
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use async_std::channel;

    use crate::player::cast_lib::CastLib;
    use crate::player::ownership::OwnerToken;
    use crate::player::session::RuntimeSession;
    use crate::player::symbols::symbol_table::SymbolOwner;

    fn player_with_cast() -> (DirPlayer, SymbolTable) {
        let (tx, _rx) = channel::unbounded();
        let mut player = DirPlayer::new_with_owner(tx, OwnerToken::transitional());
        player
            .movie
            .cast_manager
            .casts
            .push(CastLib::test_external(1, 0));
        (player, SymbolTable::new())
    }

    struct ForeignPropertyHandler {
        property: Symbol,
    }

    impl VirtualScriptHandler for ForeignPropertyHandler {
        fn get_property_names(&self) -> Vec<Symbol> {
            vec![self.property.clone()]
        }
    }

    #[test]
    fn register_and_create_javascript_proxy_preserve_virtual_cast_shape() {
        let (mut player, mut symbols) = player_with_cast();
        let cast_max_before = player.movie.cast_manager.casts[0].max_member_id();
        let script_ref = VirtualScriptRegistry::register(
            &mut player,
            "JavaScriptProxy",
            Rc::new(javascript_proxy::JavascriptProxy),
        );
        let member_number = script_ref.cast_member as u32;
        assert!(member_number >= 2_000_000);
        assert!(player.movie.cast_manager.casts[0]
            .scripts
            .contains_key(&member_number));
        assert!(!player.movie.cast_manager.casts[0]
            .members
            .contains_key(&member_number));
        assert_eq!(
            player.movie.cast_manager.casts[0].max_member_id(),
            cast_max_before
        );
        assert_eq!(
            VirtualScriptRegistry::find_by_name(&player, "javascriptproxy"),
            Some(script_ref.clone())
        );

        let new_name = symbols.intern("new");
        let result = VirtualScriptRegistry::try_call_handler(
            &mut player,
            &symbols,
            &script_ref,
            None,
            new_name,
            &Vec::new(),
        )
        .unwrap()
        .unwrap();
        let instance_ref = match player.get_datum(&result) {
            Datum::ScriptInstanceRef(instance_ref) => instance_ref.clone(),
            _ => panic!("expected ScriptInstanceRef datum"),
        };
        let call_name = symbols.intern("call");
        let void_result = VirtualScriptRegistry::try_call_handler(
            &mut player,
            &symbols,
            &script_ref,
            None,
            call_name.clone(),
            &Vec::new(),
        )
        .unwrap()
        .unwrap();
        assert!(matches!(player.get_datum(&void_result), Datum::Void));
        let receiver_result = VirtualScriptRegistry::try_call_handler(
            &mut player,
            &symbols,
            &script_ref,
            Some(&instance_ref),
            call_name,
            &Vec::new(),
        )
        .unwrap()
        .unwrap();
        let returned_ref = match player.get_datum(&receiver_result) {
            Datum::ScriptInstanceRef(returned_ref) => returned_ref,
            _ => panic!("expected ScriptInstanceRef datum"),
        };
        assert_eq!(returned_ref.id(), instance_ref.id());
        let receiver_new_name = symbols.intern("new");
        let receiver_new = VirtualScriptRegistry::try_call_handler(
            &mut player,
            &symbols,
            &script_ref,
            Some(&instance_ref),
            receiver_new_name,
            &Vec::new(),
        )
        .unwrap()
        .unwrap();
        let receiver_new_ref = match player.get_datum(&receiver_new) {
            Datum::ScriptInstanceRef(receiver_new_ref) => receiver_new_ref,
            _ => panic!("expected ScriptInstanceRef datum"),
        };
        assert_eq!(receiver_new_ref.id(), instance_ref.id());

        let unknown_name = symbols.intern("unknownVirtualHandler");
        assert!(VirtualScriptRegistry::try_call_handler(
            &mut player,
            &symbols,
            &script_ref,
            None,
            unknown_name,
            &Vec::new(),
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn foreign_name_is_rejected_before_an_absent_script_or_default_lookup() {
        let (mut player, mut symbols) = player_with_cast();
        let script_ref = VirtualScriptRegistry::register(
            &mut player,
            "JavaScriptProxy",
            Rc::new(javascript_proxy::JavascriptProxy),
        );
        let mut foreign_symbols = SymbolTable::new();
        let foreign_name = foreign_symbols.intern("foreignHandler");
        let missing = CastMemberRef {
            cast_lib: 99,
            cast_member: 99,
        };

        assert!(VirtualScriptRegistry::has_script_handler(
            &player,
            &symbols,
            &missing,
            foreign_name.clone(),
        )
        .is_err());
        assert!(VirtualScriptRegistry::try_call_handler(
            &mut player,
            &symbols,
            &missing,
            None,
            foreign_name.clone(),
            &Vec::new(),
        )
        .is_err());

        let (instance_ref, _) =
            VirtualScriptRegistry::create_instance(&mut player, &symbols, &script_ref).unwrap();
        assert!(VirtualScriptRegistry::try_get_instance_prop(
            &mut player,
            &symbols,
            &instance_ref,
            foreign_name.clone(),
        )
        .is_err());
        assert!(VirtualScriptRegistry::try_set_instance_prop(
            &mut player,
            &symbols,
            &instance_ref,
            foreign_name,
            &DatumRef::Void,
        )
        .is_err());
        assert!(javascript_proxy::JavascriptProxy
            .get_prop(
                &mut player,
                &symbols,
                &instance_ref,
                foreign_symbols.intern("foreignDefaultProperty"),
            )
            .is_err());

        player.virtual_scripts.clear();
        let local_name = symbols.intern("localProperty");
        assert_eq!(
            VirtualScriptRegistry::try_get_instance_prop(
                &mut player,
                &symbols,
                &instance_ref,
                local_name,
            )
            .unwrap(),
            None
        );
        let empty_foreign_name = foreign_symbols.intern("emptyForeignProperty");
        assert!(VirtualScriptRegistry::try_get_instance_prop(
            &mut player,
            &symbols,
            &instance_ref,
            empty_foreign_name.clone(),
        )
        .is_err());
        assert!(VirtualScriptRegistry::try_set_instance_prop(
            &mut player,
            &symbols,
            &instance_ref,
            empty_foreign_name,
            &DatumRef::Void,
        )
        .is_err());
    }

    #[test]
    fn foreign_property_names_fail_before_instance_or_datum_allocation() {
        let (mut player, local_symbols) = player_with_cast();
        let mut foreign_symbols = SymbolTable::new();
        let foreign_property = foreign_symbols.intern("foreignProperty");
        let script_ref = VirtualScriptRegistry::register(
            &mut player,
            "ForeignPropertyScript",
            Rc::new(ForeignPropertyHandler {
                property: foreign_property,
            }),
        );
        let instances_before = player.allocator.script_instance_count();
        let datums_before = player.allocator.datum_count();
        assert!(
            VirtualScriptRegistry::create_instance(&mut player, &local_symbols, &script_ref,)
                .is_err()
        );
        assert_eq!(player.allocator.script_instance_count(), instances_before);
        assert_eq!(player.allocator.datum_count(), datums_before);
    }

    #[test]
    fn inherited_receiver_can_call_an_explicit_ancestor_script_handler() {
        let (mut player, mut symbols) = player_with_cast();
        let ancestor = VirtualScriptRegistry::register(
            &mut player,
            "AncestorProxy",
            Rc::new(javascript_proxy::JavascriptProxy),
        );
        let derived = VirtualScriptRegistry::register(
            &mut player,
            "DerivedProxy",
            Rc::new(javascript_proxy::JavascriptProxy),
        );
        let (receiver, _) =
            VirtualScriptRegistry::create_instance(&mut player, &symbols, &derived).unwrap();
        let call_name = symbols.intern("call");
        let result = VirtualScriptRegistry::try_call_handler(
            &mut player,
            &symbols,
            &ancestor,
            Some(&receiver),
            call_name,
            &Vec::new(),
        )
        .unwrap()
        .unwrap();
        let returned_ref = match player.get_datum(&result) {
            Datum::ScriptInstanceRef(returned_ref) => returned_ref,
            _ => panic!("expected ScriptInstanceRef datum"),
        };
        assert_eq!(returned_ref.id(), receiver.id());
    }

    #[test]
    fn session_players_keep_virtual_instances_separate() {
        let mut session = RuntimeSession::new(SymbolOwner {
            session: 91,
            generation: 1,
        });
        let (tx_a, _rx_a) = channel::unbounded();
        let (tx_b, _rx_b) = channel::unbounded();
        assert!(session.add_player(1, tx_a));
        assert!(session.add_player(2, tx_b));
        session
            .with_player(1, |ctx| {
                ctx.player
                    .movie
                    .cast_manager
                    .casts
                    .push(CastLib::test_external(1, 0))
            })
            .unwrap();
        session
            .with_player(2, |ctx| {
                ctx.player
                    .movie
                    .cast_manager
                    .casts
                    .push(CastLib::test_external(1, 0))
            })
            .unwrap();

        let ref_a = session
            .with_player(1, |ctx| {
                let a = VirtualScriptRegistry::register(
                    ctx.player,
                    "JavaScriptProxy",
                    Rc::new(javascript_proxy::JavascriptProxy),
                );
                a
            })
            .unwrap();
        let call_name = session.symbols_mut().intern("call");
        let foreign_instance = session
            .with_player(1, |ctx| {
                VirtualScriptRegistry::create_instance(ctx.player, ctx.symbols, &ref_a)
                    .unwrap()
                    .0
            })
            .unwrap();
        session
            .with_player(2, |ctx| {
                let local_ref = VirtualScriptRegistry::register(
                    ctx.player,
                    "JavaScriptProxy",
                    Rc::new(javascript_proxy::JavascriptProxy),
                );
                assert_eq!(local_ref, ref_a);
                assert_eq!(
                    VirtualScriptRegistry::has_instance_handler(
                        ctx.player,
                        ctx.symbols,
                        &foreign_instance,
                        call_name.clone(),
                    )
                    .unwrap(),
                    false
                );
                assert!(VirtualScriptRegistry::try_call_instance_handler(
                    ctx.player,
                    ctx.symbols,
                    &foreign_instance,
                    call_name.clone(),
                    &Vec::new(),
                )
                .is_err());
                assert!(VirtualScriptRegistry::try_get_instance_prop(
                    ctx.player,
                    ctx.symbols,
                    &foreign_instance,
                    call_name,
                )
                .is_err());
            })
            .unwrap();
    }
}
