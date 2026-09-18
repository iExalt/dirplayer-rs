//! Iterative duplication of an owned Datum graph.
//!
//! `duplicate` historically lived in `mod.rs` and recursively rebuilt lists.
//! This module keeps the legacy leaf and bitmap behavior while making the
//! container walk explicit: ownership is checked first, cycles are rejected,
//! and a deep acyclic graph never consumes the Rust call stack.

use std::collections::HashSet;

use crate::director::lingo::datum::{Datum, DatumType};

use super::{
    allocator::DatumAllocatorTrait, datum_ref::DatumRef, driver,
    symbols::symbol_table::SymbolTable, DirPlayer, ScriptError, ScriptErrorCode,
};

fn invalid_reference(message: impl Into<String>) -> ScriptError {
    ScriptError::new_code(ScriptErrorCode::InvalidReference, message.into())
}

/// Reject active-path cycles in the containers that duplication expands.
///
/// Other retained references, including the source of a StringChunk and the
/// callback fields of a TimeoutInstance, are validated by the shared owner
/// graph validator but are intentionally cloned as leaves here, matching the
/// old duplicate semantics.
fn validate_container_graph(player: &DirPlayer, root: &DatumRef) -> Result<(), ScriptError> {
    enum Frame {
        Enter(DatumRef),
        Leave(usize),
    }

    let mut active = HashSet::new();
    let mut work = vec![Frame::Enter(root.clone())];

    while let Some(frame) = work.pop() {
        match frame {
            Frame::Leave(id) => {
                active.remove(&id);
            }
            Frame::Enter(reference) => {
                if matches!(reference, DatumRef::Void) {
                    continue;
                }
                let datum = player.allocator.try_get_datum(&reference).ok_or_else(|| {
                    invalid_reference(format!("invalid datum reference {reference}"))
                })?;
                match datum {
                    Datum::List(_, items, _) => {
                        let id = reference.unwrap();
                        if !active.insert(id) {
                            return Err(invalid_reference(
                                "duplicate cannot copy a cyclic list/property graph",
                            ));
                        }
                        work.push(Frame::Leave(id));
                        for child in items.iter().rev() {
                            work.push(Frame::Enter(child.clone()));
                        }
                    }
                    Datum::PropList(entries, _) => {
                        let id = reference.unwrap();
                        if !active.insert(id) {
                            return Err(invalid_reference(
                                "duplicate cannot copy a cyclic list/property graph",
                            ));
                        }
                        work.push(Frame::Leave(id));
                        for (key, value) in entries.iter().rev() {
                            work.push(Frame::Enter(value.clone()));
                            work.push(Frame::Enter(key.clone()));
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(())
}

enum DuplicateFrame {
    Enter(DatumRef),
    FinishList {
        list_type: DatumType,
        sorted: bool,
        child_count: usize,
    },
    FinishPropList {
        sorted: bool,
        child_count: usize,
    },
}

fn cleanup_failed_duplicate(
    player: &mut DirPlayer,
    created: &mut Vec<DatumRef>,
    results: &mut Vec<DatumRef>,
) {
    results.clear();
    created.clear();
    // Representable allocator failures leave owned references queued for
    // deferred reclamation. OOM/panic rollback is outside this API's claim.
    player.allocator.drain_reclaims(&mut player.bitmap_manager);
}

fn allocate_duplicate(
    player: &mut DirPlayer,
    datum: Datum,
    created: &mut Vec<DatumRef>,
) -> Result<DatumRef, ScriptError> {
    player.allocator.drain_reclaims(&mut player.bitmap_manager);
    let reference = player
        .allocator
        .alloc_datum(datum, &mut player.bitmap_manager)?;
    created.push(reference.clone());
    Ok(reference)
}

fn duplicate_leaf(
    player: &mut DirPlayer,
    source: &DatumRef,
    created: &mut Vec<DatumRef>,
) -> Result<DatumRef, ScriptError> {
    let datum = player
        .allocator
        .try_get_datum(source)
        .cloned()
        .ok_or_else(|| invalid_reference(format!("invalid datum reference {source}")))?;

    let copy = match datum {
        Datum::BitmapRef(bitmap_handle) => {
            let bitmap = player
                .bitmap_manager
                .get_bitmap_handle(&bitmap_handle)
                .cloned()
                .ok_or_else(|| invalid_reference("foreign or stale bitmap handle"))?;
            let copied_handle = player
                .bitmap_manager
                .add_ephemeral_bitmap_handle(bitmap)
                .map_err(|error| {
                    ScriptError::new(format!("bitmap allocation failed: {error:?}"))
                })?;
            Datum::BitmapRef(copied_handle)
        }
        other => other,
    };

    allocate_duplicate(player, copy, created)
}

/// Duplicate one owned datum, preserving legacy per-occurrence copy behavior.
///
/// The source graph is validated before any destination allocation. List and
/// PropList cycles are rejected because this operation expands those edges;
/// repeated children are deliberately copied independently rather than being
/// memoized, as in the existing Director behavior.
pub(crate) fn duplicate_datum(
    player: &mut DirPlayer,
    symbols: &SymbolTable,
    root: &DatumRef,
) -> Result<DatumRef, ScriptError> {
    driver::validate_owned_datum_graph(player, symbols, root)?;
    validate_container_graph(player, root)?;

    if matches!(root, DatumRef::Void) {
        return Ok(DatumRef::Void);
    }

    let mut work = vec![DuplicateFrame::Enter(root.clone())];
    let mut results = Vec::new();
    let mut created = Vec::new();

    let result = (|| -> Result<DatumRef, ScriptError> {
        while let Some(frame) = work.pop() {
            match frame {
                DuplicateFrame::Enter(source) => {
                    if matches!(source, DatumRef::Void) {
                        results.push(DatumRef::Void);
                        continue;
                    }
                    let datum = player
                        .allocator
                        .try_get_datum(&source)
                        .cloned()
                        .ok_or_else(|| {
                            invalid_reference(format!("invalid datum reference {source}"))
                        })?;
                    match datum {
                        Datum::List(list_type, items, sorted) => {
                            let child_count = items.len();
                            work.push(DuplicateFrame::FinishList {
                                list_type,
                                sorted,
                                child_count,
                            });
                            for child in items.into_iter().rev() {
                                work.push(DuplicateFrame::Enter(child));
                            }
                        }
                        Datum::PropList(entries, sorted) => {
                            let child_count = entries.len() * 2;
                            work.push(DuplicateFrame::FinishPropList {
                                sorted,
                                child_count,
                            });
                            for (key, value) in entries.into_iter().rev() {
                                work.push(DuplicateFrame::Enter(value));
                                work.push(DuplicateFrame::Enter(key));
                            }
                        }
                        _ => results.push(duplicate_leaf(player, &source, &mut created)?),
                    }
                }
                DuplicateFrame::FinishList {
                    list_type,
                    sorted,
                    child_count,
                } => {
                    let start = results
                        .len()
                        .checked_sub(child_count)
                        .ok_or_else(|| invalid_reference("duplicate list worklist underflow"))?;
                    let items = results.split_off(start);
                    results.push(allocate_duplicate(
                        player,
                        Datum::List(list_type, items.into_iter().collect(), sorted),
                        &mut created,
                    )?);
                }
                DuplicateFrame::FinishPropList {
                    sorted,
                    child_count,
                } => {
                    if child_count % 2 != 0 || results.len() < child_count {
                        return Err(invalid_reference(
                            "duplicate property-list worklist underflow",
                        ));
                    }
                    let start = results.len() - child_count;
                    let children: Vec<_> = results.split_off(start);
                    let mut entries = std::collections::VecDeque::with_capacity(child_count / 2);
                    for pair in children.chunks_exact(2) {
                        entries.push_back((pair[0].clone(), pair[1].clone()));
                    }
                    results.push(allocate_duplicate(
                        player,
                        Datum::PropList(entries, sorted),
                        &mut created,
                    )?);
                }
            }
        }

        results
            .pop()
            .ok_or_else(|| invalid_reference("duplicate produced no datum"))
    })();

    match result {
        Ok(reference) => Ok(reference),
        Err(error) => {
            cleanup_failed_duplicate(player, &mut created, &mut results);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::director::lingo::datum::Datum;
    use crate::player::bitmap::bitmap::{Bitmap, PaletteRef};
    use crate::player::ownership::{OwnerKey, OwnerToken};
    use async_std::channel;

    fn test_player(player_id: u64) -> DirPlayer {
        let (tx, _rx) = channel::unbounded();
        DirPlayer::new_with_owner(
            tx,
            OwnerToken::new(OwnerKey {
                session: 1,
                player: player_id,
                generation: 1,
            }),
        )
    }

    fn empty_list(player: &mut DirPlayer) -> DatumRef {
        player.alloc_datum(Datum::List(
            DatumType::List,
            std::collections::VecDeque::new(),
            false,
        ))
    }

    #[test]
    fn duplicate_rejects_self_cycle_before_destination_mutation() {
        let mut player = test_player(1);
        let symbols = SymbolTable::new();
        let cycle = empty_list(&mut player);
        if let Datum::List(_, items, _) = player.get_datum_mut(&cycle) {
            items.push_back(cycle.clone());
        }
        let before = player.allocator.datum_count();

        let result = duplicate_datum(&mut player, &symbols, &cycle);

        assert_eq!(
            result.err().map(|error| error.code),
            Some(ScriptErrorCode::InvalidReference)
        );
        assert_eq!(player.allocator.datum_count(), before);
    }

    #[test]
    fn duplicate_rejects_indirect_list_proplist_cycle() {
        let mut player = test_player(1);
        let symbols = SymbolTable::new();
        let list = empty_list(&mut player);
        let key = player.alloc_datum(Datum::String("cycle".to_owned()));
        let prop = player.alloc_datum(Datum::PropList(std::collections::VecDeque::new(), false));
        if let Datum::List(_, items, _) = player.get_datum_mut(&list) {
            items.push_back(prop.clone());
        }
        if let Datum::PropList(entries, _) = player.get_datum_mut(&prop) {
            entries.push_back((key, list.clone()));
        }
        let existing = player.alloc_datum(Datum::String("unchanged".to_owned()));
        let before = player.allocator.datum_count();

        let result = duplicate_datum(&mut player, &symbols, &list);

        assert_eq!(
            result.err().map(|error| error.code),
            Some(ScriptErrorCode::InvalidReference)
        );
        assert_eq!(player.allocator.datum_count(), before);
        assert!(
            matches!(player.get_datum(&existing), Datum::String(value) if value == "unchanged")
        );
    }

    #[test]
    fn duplicate_preserves_void_children_and_sorted_proplist_order() {
        let mut player = test_player(1);
        let symbols = SymbolTable::new();
        let first_key = player.alloc_datum(Datum::String("first".to_owned()));
        let second_key = player.alloc_datum(Datum::String("second".to_owned()));
        let first_value = player.alloc_datum(Datum::String("one".to_owned()));
        let second_value = player.alloc_datum(Datum::String("two".to_owned()));
        let prop = player.alloc_datum(Datum::PropList(
            std::collections::VecDeque::from([
                (first_key, first_value),
                (second_key, second_value),
            ]),
            true,
        ));
        let void_prop = player.alloc_datum(Datum::PropList(
            std::collections::VecDeque::from([(DatumRef::Void, DatumRef::Void)]),
            false,
        ));
        let root = player.alloc_datum(Datum::List(
            DatumType::List,
            std::collections::VecDeque::from([prop, void_prop, DatumRef::Void]),
            false,
        ));

        let copy = duplicate_datum(&mut player, &symbols, &root).expect("copy");
        let Datum::List(_, items, _) = player.get_datum(&copy) else {
            panic!("copy is not a list");
        };
        assert_eq!(items[2], DatumRef::Void);
        let Datum::PropList(entries, sorted) = player.get_datum(&items[0]) else {
            panic!("nested copy is not a property list");
        };
        assert!(sorted);
        assert_eq!(entries.len(), 2);
        assert!(
            matches!(player.get_datum(&entries[0].0), Datum::String(value) if value == "first")
        );
        assert!(matches!(player.get_datum(&entries[0].1), Datum::String(value) if value == "one"));
        assert!(
            matches!(player.get_datum(&entries[1].0), Datum::String(value) if value == "second")
        );
        assert!(matches!(player.get_datum(&entries[1].1), Datum::String(value) if value == "two"));
        let Datum::PropList(void_entries, sorted) = player.get_datum(&items[1]) else {
            panic!("void copy is not a property list");
        };
        assert!(!sorted);
        assert_eq!(void_entries.len(), 1);
        assert_eq!(void_entries[0].0, DatumRef::Void);
        assert_eq!(void_entries[0].1, DatumRef::Void);
    }

    #[test]
    fn duplicate_deep_acyclic_list_is_iterative() {
        let mut player = test_player(1);
        let symbols = SymbolTable::new();
        let mut current = player.alloc_datum(Datum::String("leaf".to_owned()));
        for _ in 0..1024 {
            current = player.alloc_datum(Datum::List(
                DatumType::List,
                std::collections::VecDeque::from([current]),
                false,
            ));
        }

        let copy = duplicate_datum(&mut player, &symbols, &current).expect("deep copy");
        let mut cursor = copy;
        for _ in 0..1024 {
            let Datum::List(_, items, _) = player.get_datum(&cursor) else {
                panic!("missing nested list");
            };
            cursor = items.front().cloned().expect("nested item");
        }
        assert!(matches!(player.get_datum(&cursor), Datum::String(value) if value == "leaf"));
    }

    #[test]
    fn duplicate_copies_repeated_children_per_occurrence() {
        let mut player = test_player(1);
        let symbols = SymbolTable::new();
        let child = player.alloc_datum(Datum::String("child".to_owned()));
        let root = player.alloc_datum(Datum::List(
            DatumType::List,
            std::collections::VecDeque::from([child.clone(), child]),
            false,
        ));

        let copy = duplicate_datum(&mut player, &symbols, &root).expect("copy");
        let Datum::List(_, items, _) = player.get_datum(&copy) else {
            panic!("copy is not a list");
        };
        assert_eq!(items.len(), 2);
        assert_ne!(items[0], items[1]);
        assert_ne!(items[0], player.get_datum(&root).to_list().unwrap()[0]);
    }

    #[test]
    fn duplicate_bitmap_copy_is_independent() {
        let mut player = test_player(1);
        let symbols = SymbolTable::new();
        let source_handle = player
            .bitmap_manager
            .add_ephemeral_bitmap_handle(Bitmap::new(1, 1, 32, 32, 8, PaletteRef::Default))
            .expect("source bitmap");
        player
            .bitmap_manager
            .get_bitmap_handle_mut(&source_handle)
            .expect("source handle")
            .data
            .copy_from_slice(&[10, 20, 30, 255]);
        let source = player.alloc_datum(Datum::BitmapRef(source_handle.clone()));

        let copy = duplicate_datum(&mut player, &symbols, &source).expect("bitmap copy");
        let copy_handle = match player.get_datum(&copy) {
            Datum::BitmapRef(copy_handle) => copy_handle.clone(),
            _ => panic!("copy is not a bitmap"),
        };
        assert_ne!(copy_handle, source_handle);
        player
            .bitmap_manager
            .get_bitmap_handle_mut(&copy_handle)
            .expect("copy handle")
            .data
            .copy_from_slice(&[90, 80, 70, 255]);
        assert_eq!(
            player
                .bitmap_manager
                .get_bitmap_handle(&source_handle)
                .expect("source handle")
                .data,
            vec![10, 20, 30, 255]
        );
    }
}
