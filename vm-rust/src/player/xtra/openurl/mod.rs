//! OpenURL Xtra (Gary Smith, 1997) — single global handler that opens a URL in
//! the default browser.
//!
//! Lingo signature:
//!   `gsOpenURL string URL` -> integer 1 on success, 0 on failure.
//!
//! In a browser host the "default browser" is already this page; `window.open`
//! either pops a new tab (success) or is blocked by the popup blocker
//! (failure).

use crate::player::{symbols::symbol_table::SymbolTable, DatumRef, DirPlayer, ScriptError};

#[derive(Clone, Debug)]
pub(crate) struct OpenUrlHostIntent {
    pub(crate) owner: crate::player::ownership::OwnerToken,
    pub(crate) url: String,
}

pub struct OpenUrlXtra;

impl OpenUrlXtra {
    pub fn has_handler(name: &str) -> bool {
        name.eq_ignore_ascii_case("gsOpenURL")
    }

    pub(crate) fn prepare_handler(
        player: &mut DirPlayer,
        name: &str,
        args: &[DatumRef],
        symbols: &SymbolTable,
    ) -> Result<crate::player::xtra::manager::XtraPendingOrValue, ScriptError> {
        if !Self::has_handler(name) {
            return Err(ScriptError::new(format!("OpenURL: no handler {}", name)));
        }
        let arg = args
            .first()
            .ok_or_else(|| ScriptError::new("gsOpenURL requires a URL argument".to_owned()))?;
        let url = player
            .allocator
            .try_get_datum(arg)
            .ok_or_else(|| {
                ScriptError::new_code(
                    crate::player::ScriptErrorCode::InvalidReference,
                    "foreign or stale OpenURL argument".to_owned(),
                )
            })?
            .string_value(symbols)?;
        Ok(crate::player::xtra::manager::XtraPendingOrValue::Pending(
            crate::player::xtra::manager::XtraPendingIntent::OpenUrl(OpenUrlHostIntent {
                owner: player.owner.clone(),
                url,
            }),
        ))
    }
}
