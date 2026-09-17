//! SysMenu Xtra (v0.3, 2025 59de44955ebd) — manipulate Director's
//! host-window system menu bar.
//!
//! Browser menu state is deliberately owned by one player. The browser has
//! no native menu bar, so menu mutations remain an in-memory model while
//! print/message-box effects cross the normal owner-bound host boundary.

use fxhash::FxHashMap;

use crate::{
    director::lingo::datum::Datum,
    player::{
        ownership::OwnerToken, symbols::symbol_table::SymbolTable, DatumRef, DirPlayer,
        ScriptError, ScriptErrorCode,
    },
};

#[derive(Clone)]
struct MenuItem {
    name: String,
    id: i32,
    checked: bool,
    enabled: bool,
    sub_menu_pos: Option<i32>,
}

#[derive(Clone)]
struct SysMenuState {
    items: Vec<MenuItem>,
    by_id: FxHashMap<i32, usize>,
    dark_mode: bool,
}

impl SysMenuState {
    fn new() -> Self {
        Self {
            items: Vec::new(),
            by_id: FxHashMap::default(),
            dark_mode: false,
        }
    }

    fn reindex(&mut self) {
        self.by_id.clear();
        for (index, item) in self.items.iter().enumerate() {
            if item.id != 0 {
                self.by_id.insert(item.id, index);
            }
        }
    }
}

/// SysMenu state and capability are scoped to one player owner. No menu
/// operation consults a process-global state slot.
pub(crate) struct SysMenuManager {
    pub(crate) owner: OwnerToken,
    state: SysMenuState,
}

impl SysMenuManager {
    pub(crate) fn new(owner: OwnerToken) -> Self {
        Self {
            owner,
            state: SysMenuState::new(),
        }
    }

    pub(crate) fn reset(&mut self) {
        self.state = SysMenuState::new();
    }

    pub(crate) fn rebind_owner(&mut self, owner: OwnerToken) {
        self.owner = owner;
        self.reset();
    }

    #[cfg(test)]
    pub(crate) fn item_count(&self) -> usize {
        self.state.items.len()
    }

    #[cfg(test)]
    pub(crate) fn dark_mode(&self) -> bool {
        self.state.dark_mode
    }
}

#[derive(Clone)]
pub(crate) enum SysMenuHostIntent {
    Print {
        owner: OwnerToken,
        message: String,
    },
    MessageBox {
        owner: OwnerToken,
        message: String,
        caption: String,
        message_type: i32,
    },
}

impl SysMenuHostIntent {
    pub(crate) fn owner(&self) -> &OwnerToken {
        match self {
            Self::Print { owner, .. } | Self::MessageBox { owner, .. } => owner,
        }
    }
}

pub struct SysMenuXtra;

impl SysMenuXtra {
    pub fn has_handler(name: &str) -> bool {
        matches!(
            name.to_ascii_lowercase().as_str(),
            "sysmenuinsertmenu"
                | "sysmenuinsertitem"
                | "sysmenuinsertseparator"
                | "sysmenucheckitem"
                | "sysmenuenableitem"
                | "sysmenuremoveitem"
                | "sysmenuusedarkmode"
                | "sysmenuprintmsg"
                | "sysmenumessagebox"
        )
    }

    /// Execute only synchronous menu-model handlers. Host effects must use
    /// `prepare_handler` so the command pump can release RuntimeSession.
    pub(crate) fn call_handler_explicit(
        manager: &mut SysMenuManager,
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        name: &str,
        args: &[DatumRef],
    ) -> Result<DatumRef, ScriptError> {
        ensure_owner(manager, player)?;
        match_ci!(name, {
            "sysMenuInsertMenu" => insert_menu(manager, player, symbols, args),
            "sysMenuInsertItem" => insert_item(manager, player, symbols, args),
            "sysMenuInsertSeparator" => insert_separator(manager, player, symbols, args),
            "sysMenuCheckItem" => check_item(manager, player, symbols, args),
            "sysMenuEnableItem" => enable_item(manager, player, symbols, args),
            "sysMenuRemoveItem" => remove_item(manager, player, symbols, args),
            "sysMenuUseDarkMode" => use_dark_mode(manager, player, symbols, args),
            "sysMenuPrintMsg" | "sysMenuMessageBox" => Err(ScriptError::new(
                "SysMenu host effect requires the owner-bound executor".to_owned())),
            _ => Err(ScriptError::new(format!("SysMenu: no handler {}", name))),
        })
    }

    pub(crate) fn prepare_handler(
        manager: &mut SysMenuManager,
        player: &mut DirPlayer,
        symbols: &SymbolTable,
        name: &str,
        args: &[DatumRef],
    ) -> Result<super::manager::XtraPendingOrValue, ScriptError> {
        ensure_owner(manager, player)?;
        match_ci!(name, {
            "sysMenuPrintMsg" => {
                let message = string_arg(player, args, 0, "sysMenuPrintMsg", symbols)?;
                Ok(super::manager::XtraPendingOrValue::Pending(
                    super::manager::XtraPendingIntent::SysMenu(
                        SysMenuHostIntent::Print { owner: manager.owner.clone(), message })))
            },
            "sysMenuMessageBox" => {
                let message = string_arg(player, args, 0, "sysMenuMessageBox", symbols)?;
                let caption = if args.get(1).is_some() {
                    string_arg(player, args, 1, "sysMenuMessageBox", symbols)?
                } else { String::new() };
                let message_type = optional_int(player, args, 2, symbols)?.unwrap_or(0);
                Ok(super::manager::XtraPendingOrValue::Pending(
                    super::manager::XtraPendingIntent::SysMenu(
                        SysMenuHostIntent::MessageBox {
                            owner: manager.owner.clone(), message, caption, message_type,
                        })))
            },
            _ => Self::call_handler_explicit(manager, player, symbols, name, args)
                .map(super::manager::XtraPendingOrValue::Value),
        })
    }
}

fn ensure_owner(manager: &SysMenuManager, player: &DirPlayer) -> Result<(), ScriptError> {
    if !manager.owner.same_identity(&player.owner)
        || !manager.owner.is_arena_live()
        || !player.owner.is_arena_live()
    {
        return Err(ScriptError::new_code(
            ScriptErrorCode::InvalidReference,
            "foreign or stale SysMenu owner".to_owned(),
        ));
    }
    Ok(())
}

fn invalid_ref() -> ScriptError {
    ScriptError::new_code(
        ScriptErrorCode::InvalidReference,
        "foreign or stale SysMenu argument".to_owned(),
    )
}

fn checked_datum<'a>(player: &'a DirPlayer, arg: &DatumRef) -> Result<&'a Datum, ScriptError> {
    player.allocator.try_get_datum(arg).ok_or_else(invalid_ref)
}

fn int_arg(
    player: &DirPlayer,
    args: &[DatumRef],
    idx: usize,
    name: &str,
    _symbols: &SymbolTable,
) -> Result<i32, ScriptError> {
    let arg = args.get(idx).ok_or_else(|| {
        ScriptError::new(format!(
            "{} requires argument at position {}",
            name,
            idx + 1
        ))
    })?;
    checked_datum(player, arg)?.int_value()
}

fn string_arg(
    player: &DirPlayer,
    args: &[DatumRef],
    idx: usize,
    name: &str,
    symbols: &SymbolTable,
) -> Result<String, ScriptError> {
    let arg = args.get(idx).ok_or_else(|| {
        ScriptError::new(format!(
            "{} requires argument at position {}",
            name,
            idx + 1
        ))
    })?;
    checked_datum(player, arg)?.string_value(symbols)
}

fn optional_int(
    player: &DirPlayer,
    args: &[DatumRef],
    idx: usize,
    _symbols: &SymbolTable,
) -> Result<Option<i32>, ScriptError> {
    let Some(arg) = args.get(idx) else {
        return Ok(None);
    };
    Ok(checked_datum(player, arg)?.int_value().ok())
}

fn ok_int(player: &mut DirPlayer, n: i32) -> Result<DatumRef, ScriptError> {
    Ok(player.alloc_datum(Datum::Int(n)))
}

fn insert_menu(
    manager: &mut SysMenuManager,
    player: &mut DirPlayer,
    symbols: &SymbolTable,
    args: &[DatumRef],
) -> Result<DatumRef, ScriptError> {
    let position = int_arg(player, args, 0, "sysMenuInsertMenu", symbols)?;
    let name = string_arg(player, args, 1, "sysMenuInsertMenu", symbols)?;
    let sub = optional_int(player, args, 2, symbols)?;
    let idx = ((position - 1).max(0) as usize).min(manager.state.items.len());
    manager.state.items.insert(
        idx,
        MenuItem {
            name,
            id: 0,
            checked: false,
            enabled: true,
            sub_menu_pos: sub,
        },
    );
    manager.state.reindex();
    ok_int(player, 1)
}

fn insert_item(
    manager: &mut SysMenuManager,
    player: &mut DirPlayer,
    symbols: &SymbolTable,
    args: &[DatumRef],
) -> Result<DatumRef, ScriptError> {
    let position = int_arg(player, args, 0, "sysMenuInsertItem", symbols)?;
    let name = string_arg(player, args, 1, "sysMenuInsertItem", symbols)?;
    let id = int_arg(player, args, 2, "sysMenuInsertItem", symbols)?;
    let sub = optional_int(player, args, 3, symbols)?;
    let idx = ((position - 1).max(0) as usize).min(manager.state.items.len());
    manager.state.items.insert(
        idx,
        MenuItem {
            name,
            id,
            checked: false,
            enabled: true,
            sub_menu_pos: sub,
        },
    );
    manager.state.reindex();
    ok_int(player, 1)
}

fn insert_separator(
    manager: &mut SysMenuManager,
    player: &mut DirPlayer,
    symbols: &SymbolTable,
    args: &[DatumRef],
) -> Result<DatumRef, ScriptError> {
    let position = int_arg(player, args, 0, "sysMenuInsertSeparator", symbols)?;
    let sub = optional_int(player, args, 1, symbols)?;
    let idx = ((position - 1).max(0) as usize).min(manager.state.items.len());
    manager.state.items.insert(
        idx,
        MenuItem {
            name: "-".to_owned(),
            id: 0,
            checked: false,
            enabled: true,
            sub_menu_pos: sub,
        },
    );
    manager.state.reindex();
    ok_int(player, 1)
}

fn check_item(
    manager: &mut SysMenuManager,
    player: &mut DirPlayer,
    symbols: &SymbolTable,
    args: &[DatumRef],
) -> Result<DatumRef, ScriptError> {
    let id = int_arg(player, args, 0, "sysMenuCheckItem", symbols)?;
    let checked = int_arg(player, args, 1, "sysMenuCheckItem", symbols)? != 0;
    let found = manager
        .state
        .by_id
        .get(&id)
        .copied()
        .map(|idx| {
            manager.state.items[idx].checked = checked;
            true
        })
        .unwrap_or(false);
    ok_int(player, if found { 1 } else { 0 })
}

fn enable_item(
    manager: &mut SysMenuManager,
    player: &mut DirPlayer,
    symbols: &SymbolTable,
    args: &[DatumRef],
) -> Result<DatumRef, ScriptError> {
    let id = int_arg(player, args, 0, "sysMenuEnableItem", symbols)?;
    let enabled = int_arg(player, args, 1, "sysMenuEnableItem", symbols)? != 0;
    let found = manager
        .state
        .by_id
        .get(&id)
        .copied()
        .map(|idx| {
            manager.state.items[idx].enabled = enabled;
            true
        })
        .unwrap_or(false);
    ok_int(player, if found { 1 } else { 0 })
}

fn remove_item(
    manager: &mut SysMenuManager,
    player: &mut DirPlayer,
    symbols: &SymbolTable,
    args: &[DatumRef],
) -> Result<DatumRef, ScriptError> {
    let id = int_arg(player, args, 0, "sysMenuRemoveItem", symbols)?;
    let removed = if let Some(idx) = manager.state.by_id.get(&id).copied() {
        manager.state.items.remove(idx);
        manager.state.reindex();
        true
    } else {
        false
    };
    ok_int(player, if removed { 1 } else { 0 })
}

fn use_dark_mode(
    manager: &mut SysMenuManager,
    player: &mut DirPlayer,
    symbols: &SymbolTable,
    args: &[DatumRef],
) -> Result<DatumRef, ScriptError> {
    manager.state.dark_mode = optional_int(player, args, 0, symbols)?.unwrap_or(1) != 0;
    ok_int(player, 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_std::channel;

    fn player(owner: OwnerToken) -> DirPlayer {
        let (tx, _rx) = channel::unbounded();
        DirPlayer::new_with_owner(tx, owner)
    }

    #[test]
    fn menu_state_is_owner_local_and_reset_does_not_touch_other_owner() {
        let owner_a = OwnerToken::transitional();
        let owner_b = OwnerToken::transitional();
        let mut player_a = player(owner_a.clone());
        let mut player_b = player(owner_b.clone());
        let mut manager_a = SysMenuManager::new(owner_a);
        let mut manager_b = SysMenuManager::new(owner_b);
        let symbols = SymbolTable::new();

        let args_a = vec![
            player_a.alloc_datum(Datum::Int(1)),
            player_a.alloc_datum(Datum::String("A".to_owned())),
            player_a.alloc_datum(Datum::Int(11)),
        ];
        let args_b = vec![
            player_b.alloc_datum(Datum::Int(1)),
            player_b.alloc_datum(Datum::String("B".to_owned())),
            player_b.alloc_datum(Datum::Int(11)),
        ];
        SysMenuXtra::call_handler_explicit(
            &mut manager_a,
            &mut player_a,
            &symbols,
            "sysMenuInsertItem",
            &args_a,
        )
        .unwrap();
        SysMenuXtra::call_handler_explicit(
            &mut manager_b,
            &mut player_b,
            &symbols,
            "sysMenuInsertItem",
            &args_b,
        )
        .unwrap();
        assert_eq!(manager_a.item_count(), 1);
        assert_eq!(manager_b.item_count(), 1);

        manager_a.reset();
        assert_eq!(manager_a.item_count(), 0);
        assert_eq!(manager_b.item_count(), 1);
    }

    #[test]
    fn foreign_argument_is_rejected_before_menu_mutation() {
        let owner_a = OwnerToken::transitional();
        let owner_b = OwnerToken::transitional();
        let mut player_a = player(owner_a.clone());
        let mut player_b = player(owner_b.clone());
        let mut manager_b = SysMenuManager::new(owner_b);
        let symbols = SymbolTable::new();
        let foreign = vec![player_a.alloc_datum(Datum::Int(1))];
        let error = SysMenuXtra::call_handler_explicit(
            &mut manager_b,
            &mut player_b,
            &symbols,
            "sysMenuInsertSeparator",
            &foreign,
        )
        .unwrap_err();
        assert_eq!(error.code, ScriptErrorCode::InvalidReference);
        assert_eq!(manager_b.item_count(), 0);
    }

    #[test]
    fn manager_owner_must_match_player_before_dispatch() {
        let manager_owner = OwnerToken::transitional();
        let player_owner = OwnerToken::transitional();
        let mut manager = SysMenuManager::new(manager_owner);
        let mut player = player(player_owner);
        let symbols = SymbolTable::new();
        let error = SysMenuXtra::call_handler_explicit(
            &mut manager,
            &mut player,
            &symbols,
            "sysMenuUseDarkMode",
            &[],
        )
        .unwrap_err();
        assert_eq!(error.code, ScriptErrorCode::InvalidReference);
    }
}
