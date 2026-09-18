//! Explicit owner-to-owner transfer for value-only Director data.
//!
//! A `DatumRef` is an arena capability, so cloning a `Datum` that contains
//! child refs is not a transfer operation.  This module snapshots an owned
//! graph first, then imports that snapshot into a destination allocator.  The
//! snapshot contains no source arena refs, bitmap handles, object handles, or
//! source symbols.

use std::collections::{HashMap, HashSet};

use crate::director::lingo::datum::{Datum, DatumType, StringChunkExpr, StringChunkSource};

use super::{
    allocator::DatumAllocatorTrait,
    datum_ref::{DatumId, DatumRef},
    driver,
    sprite::ColorRef,
    symbols::{builtin::BuiltInSymbol, symbol_table::SymbolTable},
    DirPlayer,
};

/// A stable index into an owned snapshot.  It is deliberately separate from
/// `DatumId`: importing a snapshot must never reinterpret a source arena ID in
/// the destination arena.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct SnapshotId(usize);

#[derive(Clone, Debug)]
pub(crate) enum ValueTransferError {
    SourceOwnerDead,
    DestinationOwnerDead,
    InvalidReference(String),
    ForeignSymbol(String),
    UnsupportedDatum(DatumType),
    CyclicDatum,
    // Recoverable allocator errors clean up temporary DatumRefs best effort;
    // symbol-table intern additions and OOM/panic paths are outside rollback.
    Allocation(String),
    SnapshotCorrupt(String),
}

#[derive(Clone, Debug)]
enum SnapshotRef {
    Void,
    Node(SnapshotId),
}

#[derive(Clone, Debug)]
enum SnapshotSymbol {
    Builtin(BuiltInSymbol),
    Dynamic(String),
}

#[derive(Clone)]
enum SnapshotNode {
    Int(i32),
    Float(f64),
    String(String),
    Symbol(SnapshotSymbol),
    Null,
    Rect([f64; 4], u8),
    Point([f64; 2], u8),
    Color(ColorRef),
    Vector([f64; 3]),
    Transform3d([f64; 16]),
    JavaScript(Vec<u8>),
    StringChunk {
        source: SnapshotRef,
        expression: StringChunkExpr,
        cached_value: String,
    },
    List {
        list_type: DatumType,
        items: Vec<SnapshotRef>,
        sorted: bool,
    },
    PropList {
        entries: Vec<(SnapshotRef, SnapshotRef)>,
        sorted: bool,
    },
}

/// An owner-independent value graph.  Repeated source refs point to the same
/// `SnapshotId`; importing therefore preserves aliases without retaining any
/// source capability.
#[derive(Clone)]
pub(crate) struct OwnedValueSnapshot {
    root: SnapshotRef,
    nodes: Vec<SnapshotNode>,
}

#[derive(Clone)]
enum PendingNode {
    StringChunk {
        source: DatumRef,
        expression: StringChunkExpr,
        cached_value: String,
    },
    List {
        list_type: DatumType,
        items: Vec<DatumRef>,
        sorted: bool,
    },
    PropList {
        entries: Vec<(DatumRef, DatumRef)>,
        sorted: bool,
    },
}

#[derive(Clone)]
enum Frame {
    Enter {
        reference: DatumRef,
        is_root: bool,
    },
    Finish {
        source_id: DatumId,
        snapshot_id: SnapshotId,
        pending: PendingNode,
        is_root: bool,
    },
}

#[derive(Clone, Copy, Debug)]
enum VisitState {
    Active(SnapshotId),
    Done(SnapshotId),
}

/// Snapshot a validated source graph without allocating in another player.
///
/// This is the first phase of transfer.  It validates the source owner and
/// every retained child before any destination symbol is interned or datum is
/// allocated.  Container traversal is iterative, so a deep acyclic graph does
/// not consume the Rust call stack.  Cycles are rejected because the current
/// allocator API has no transactional placeholder operation for constructing
/// cyclic graphs.
pub(crate) fn snapshot_owned_value(
    source: &DirPlayer,
    source_symbols: &SymbolTable,
    root: &DatumRef,
) -> Result<OwnedValueSnapshot, ValueTransferError> {
    if !source.owner.is_arena_live() {
        return Err(ValueTransferError::SourceOwnerDead);
    }

    driver::validate_owned_datum_graph(source, source_symbols, root)
        .map_err(|error| ValueTransferError::InvalidReference(error.message))?;

    let mut nodes = Vec::new();
    let mut states: HashMap<DatumId, VisitState> = HashMap::new();
    let mut frames = vec![Frame::Enter {
        reference: root.clone(),
        is_root: true,
    }];
    let mut snapshot_root = None;

    while let Some(frame) = frames.pop() {
        match frame {
            Frame::Enter { reference, is_root } => {
                let SnapshotRef::Node(snapshot_id) =
                    snapshot_ref_for_source(source, &reference, &states)?
                else {
                    if is_root {
                        snapshot_root = Some(SnapshotRef::Void);
                    }
                    continue;
                };

                if let Some(state) = states.get(&reference.unwrap()).copied() {
                    match state {
                        VisitState::Active(_) => {
                            return Err(ValueTransferError::CyclicDatum);
                        }
                        VisitState::Done(done_id) => {
                            if is_root {
                                snapshot_root = Some(SnapshotRef::Node(done_id));
                            }
                            continue;
                        }
                    }
                }

                let datum = source.allocator.try_get_datum(&reference).ok_or_else(|| {
                    ValueTransferError::InvalidReference(format!(
                        "invalid source datum reference {reference}"
                    ))
                })?;
                states.insert(reference.unwrap(), VisitState::Active(snapshot_id));

                let pending = match datum {
                    Datum::StringChunk(source_ref, expression, cached_value) => {
                        let StringChunkSource::Datum(child) = source_ref else {
                            return Err(ValueTransferError::UnsupportedDatum(
                                DatumType::StringChunk,
                            ));
                        };
                        Some(PendingNode::StringChunk {
                            source: child.clone(),
                            expression: expression.clone(),
                            cached_value: cached_value.clone(),
                        })
                    }
                    Datum::List(list_type, items, sorted) => Some(PendingNode::List {
                        list_type: list_type.clone(),
                        items: items.iter().cloned().collect(),
                        sorted: *sorted,
                    }),
                    Datum::PropList(entries, sorted) => Some(PendingNode::PropList {
                        entries: entries.iter().cloned().collect(),
                        sorted: *sorted,
                    }),
                    leaf => {
                        let node = snapshot_leaf(leaf, source_symbols)?;
                        nodes.push(node);
                        states.insert(reference.unwrap(), VisitState::Done(snapshot_id));
                        if is_root {
                            snapshot_root = Some(SnapshotRef::Node(snapshot_id));
                        }
                        continue;
                    }
                };

                nodes.push(SnapshotNode::Null);
                let pending = pending.ok_or_else(|| {
                    ValueTransferError::SnapshotCorrupt(
                        "container has no pending transfer node".to_owned(),
                    )
                })?;
                frames.push(Frame::Finish {
                    source_id: reference.unwrap(),
                    snapshot_id,
                    pending: pending.clone(),
                    is_root,
                });
                push_pending_children(&mut frames, &pending);
            }
            Frame::Finish {
                source_id,
                snapshot_id,
                pending,
                is_root,
            } => {
                let node = finish_pending_node(&pending, &states)?;
                let slot = nodes.get_mut(snapshot_id.0).ok_or_else(|| {
                    ValueTransferError::SnapshotCorrupt(format!(
                        "missing snapshot slot {}",
                        snapshot_id.0
                    ))
                })?;
                *slot = node;
                states.insert(source_id, VisitState::Done(snapshot_id));
                if is_root {
                    snapshot_root = Some(SnapshotRef::Node(snapshot_id));
                }
            }
        }
    }

    Ok(OwnedValueSnapshot {
        root: snapshot_root.ok_or_else(|| {
            ValueTransferError::SnapshotCorrupt("source snapshot has no root".to_owned())
        })?,
        nodes,
    })
}

/// Snapshot and import a value when source and destination are separately
/// borrowed.  No source `DatumRef` or source `Symbol` survives the call.
pub(crate) fn copy_owned_value(
    source: &DirPlayer,
    source_symbols: &SymbolTable,
    destination: &mut DirPlayer,
    destination_symbols: &mut SymbolTable,
    root: &DatumRef,
) -> Result<DatumRef, ValueTransferError> {
    let snapshot = snapshot_owned_value(source, source_symbols, root)?;
    import_owned_value(&snapshot, destination, destination_symbols)
}

/// Import a previously validated, owner-independent snapshot.
///
/// Source rejection happens before destination allocation or interning.  If a
/// representable allocator error occurs after that preflight, temporary datum
/// refs are dropped and the destination reclaim queue is drained.  Interned
/// symbol names are intentionally not rolled back, and OOM/panic paths are
/// outside this API's transactional contract.
pub(crate) fn import_owned_value(
    snapshot: &OwnedValueSnapshot,
    destination: &mut DirPlayer,
    destination_symbols: &mut SymbolTable,
) -> Result<DatumRef, ValueTransferError> {
    if !destination.owner.is_arena_live() {
        return Err(ValueTransferError::DestinationOwnerDead);
    }
    let SnapshotRef::Node(root) = snapshot.root else {
        return Ok(DatumRef::Void);
    };

    let order = postorder(snapshot, root)?;
    let mut imported: HashMap<SnapshotId, DatumRef> = HashMap::new();

    for node_id in order {
        let datum = match materialize_node(snapshot.node(node_id)?, &imported, destination_symbols)
        {
            Ok(datum) => datum,
            Err(error) => {
                rollback_import(destination, &mut imported);
                return Err(error);
            }
        };
        let reference = match allocate_destination(destination, datum) {
            Ok(reference) => reference,
            Err(error) => {
                rollback_import(destination, &mut imported);
                return Err(error);
            }
        };
        imported.insert(node_id, reference);
    }

    let result = imported.remove(&root).ok_or_else(|| {
        ValueTransferError::SnapshotCorrupt("imported snapshot root is missing".to_owned())
    });
    if result.is_err() {
        rollback_import(destination, &mut imported);
    }
    result
}

fn snapshot_ref_for_source(
    source: &DirPlayer,
    reference: &DatumRef,
    states: &HashMap<DatumId, VisitState>,
) -> Result<SnapshotRef, ValueTransferError> {
    match reference {
        DatumRef::Void => Ok(SnapshotRef::Void),
        DatumRef::Ref(_) => {
            if source.allocator.try_get_datum(reference).is_none() {
                return Err(ValueTransferError::InvalidReference(format!(
                    "invalid source datum reference {reference}"
                )));
            }
            if let Some(state) = states.get(&reference.unwrap()) {
                match state {
                    VisitState::Active(snapshot_id) | VisitState::Done(snapshot_id) => {
                        return Ok(SnapshotRef::Node(*snapshot_id));
                    }
                }
            }
            Ok(SnapshotRef::Node(SnapshotId(states.len())))
        }
    }
}

fn push_pending_children(frames: &mut Vec<Frame>, pending: &PendingNode) {
    match pending {
        PendingNode::StringChunk { source, .. } => frames.push(Frame::Enter {
            reference: source.clone(),
            is_root: false,
        }),
        PendingNode::List { items, .. } => {
            for item in items.iter().rev() {
                frames.push(Frame::Enter {
                    reference: item.clone(),
                    is_root: false,
                });
            }
        }
        PendingNode::PropList { entries, .. } => {
            for (key, value) in entries.iter().rev() {
                frames.push(Frame::Enter {
                    reference: value.clone(),
                    is_root: false,
                });
                frames.push(Frame::Enter {
                    reference: key.clone(),
                    is_root: false,
                });
            }
        }
    }
}

fn finish_pending_node(
    pending: &PendingNode,
    states: &HashMap<DatumId, VisitState>,
) -> Result<SnapshotNode, ValueTransferError> {
    let resolve = |reference: &DatumRef| -> Result<SnapshotRef, ValueTransferError> {
        match reference {
            DatumRef::Void => Ok(SnapshotRef::Void),
            DatumRef::Ref(_) => match states.get(&reference.unwrap()) {
                Some(VisitState::Done(snapshot_id)) => Ok(SnapshotRef::Node(*snapshot_id)),
                Some(VisitState::Active(_)) => Err(ValueTransferError::CyclicDatum),
                None => Err(ValueTransferError::SnapshotCorrupt(format!(
                    "child datum {} was not visited",
                    reference.unwrap()
                ))),
            },
        }
    };

    match pending {
        PendingNode::StringChunk {
            source,
            expression,
            cached_value,
        } => Ok(SnapshotNode::StringChunk {
            source: resolve(source)?,
            expression: expression.clone(),
            cached_value: cached_value.clone(),
        }),
        PendingNode::List {
            list_type,
            items,
            sorted,
        } => Ok(SnapshotNode::List {
            list_type: list_type.clone(),
            items: items.iter().map(resolve).collect::<Result<_, _>>()?,
            sorted: *sorted,
        }),
        PendingNode::PropList { entries, sorted } => Ok(SnapshotNode::PropList {
            entries: entries
                .iter()
                .map(|(key, value)| Ok((resolve(key)?, resolve(value)?)))
                .collect::<Result<_, ValueTransferError>>()?,
            sorted: *sorted,
        }),
    }
}

fn snapshot_leaf(datum: &Datum, symbols: &SymbolTable) -> Result<SnapshotNode, ValueTransferError> {
    match datum {
        Datum::Int(value) => Ok(SnapshotNode::Int(*value)),
        Datum::Float(value) => Ok(SnapshotNode::Float(*value)),
        Datum::String(value) => Ok(SnapshotNode::String(value.clone())),
        Datum::Symbol(symbol) => {
            if let Some(builtin) = symbol.into_builtin() {
                Ok(SnapshotNode::Symbol(SnapshotSymbol::Builtin(builtin)))
            } else {
                let spelling = symbols
                    .display(symbol)
                    .map_err(|_| ValueTransferError::ForeignSymbol("foreign symbol".to_owned()))?;
                Ok(SnapshotNode::Symbol(SnapshotSymbol::Dynamic(
                    spelling.to_owned(),
                )))
            }
        }
        Datum::Null => Ok(SnapshotNode::Null),
        Datum::Rect(values, flags) => Ok(SnapshotNode::Rect(*values, *flags)),
        Datum::Point(values, flags) => Ok(SnapshotNode::Point(*values, *flags)),
        Datum::ColorRef(value) => Ok(SnapshotNode::Color(value.clone())),
        Datum::Vector(value) => Ok(SnapshotNode::Vector(*value)),
        Datum::Transform3d(value) => Ok(SnapshotNode::Transform3d(**value)),
        Datum::JavaScript(value) => Ok(SnapshotNode::JavaScript(value.clone())),
        other => Err(ValueTransferError::UnsupportedDatum(other.type_enum())),
    }
}

fn postorder(
    snapshot: &OwnedValueSnapshot,
    root: SnapshotId,
) -> Result<Vec<SnapshotId>, ValueTransferError> {
    let mut order = Vec::new();
    let mut stack = vec![(root, false)];
    let mut active = HashSet::new();
    let mut complete = HashSet::new();
    while let Some((node_id, exiting)) = stack.pop() {
        let node = snapshot.node(node_id)?;
        if exiting {
            active.remove(&node_id);
            complete.insert(node_id);
            order.push(node_id);
            continue;
        }
        if complete.contains(&node_id) {
            continue;
        }
        if !active.insert(node_id) {
            return Err(ValueTransferError::CyclicDatum);
        }
        stack.push((node_id, true));
        for child in snapshot_children(node).into_iter().rev() {
            if let SnapshotRef::Node(child_id) = child {
                if !complete.contains(&child_id) {
                    stack.push((child_id, false));
                }
            }
        }
    }
    Ok(order)
}

fn snapshot_children(node: &SnapshotNode) -> Vec<SnapshotRef> {
    match node {
        SnapshotNode::StringChunk { source, .. } => vec![source.clone()],
        SnapshotNode::List { items, .. } => items.clone(),
        SnapshotNode::PropList { entries, .. } => entries
            .iter()
            .flat_map(|(key, value)| [key.clone(), value.clone()])
            .collect(),
        _ => Vec::new(),
    }
}

fn materialize_node(
    node: &SnapshotNode,
    imported: &HashMap<SnapshotId, DatumRef>,
    symbols: &mut SymbolTable,
) -> Result<Datum, ValueTransferError> {
    let child = |reference: &SnapshotRef| -> Result<DatumRef, ValueTransferError> {
        match reference {
            SnapshotRef::Void => Ok(DatumRef::Void),
            SnapshotRef::Node(node_id) => imported.get(node_id).cloned().ok_or_else(|| {
                ValueTransferError::SnapshotCorrupt(format!(
                    "child snapshot {} was not imported",
                    node_id.0
                ))
            }),
        }
    };
    Ok(match node {
        SnapshotNode::Int(value) => Datum::Int(*value),
        SnapshotNode::Float(value) => Datum::Float(*value),
        SnapshotNode::String(value) => Datum::String(value.clone()),
        SnapshotNode::Symbol(SnapshotSymbol::Builtin(value)) => Datum::Symbol(value.clone().into()),
        SnapshotNode::Symbol(SnapshotSymbol::Dynamic(value)) => {
            Datum::Symbol(symbols.intern(value))
        }
        SnapshotNode::Null => Datum::Null,
        SnapshotNode::Rect(values, flags) => Datum::Rect(*values, *flags),
        SnapshotNode::Point(values, flags) => Datum::Point(*values, *flags),
        SnapshotNode::Color(value) => Datum::ColorRef(value.clone()),
        SnapshotNode::Vector(value) => Datum::Vector(*value),
        SnapshotNode::Transform3d(value) => Datum::transform3d(*value),
        SnapshotNode::JavaScript(value) => Datum::JavaScript(value.clone()),
        SnapshotNode::StringChunk {
            source,
            expression,
            cached_value,
        } => Datum::StringChunk(
            StringChunkSource::Datum(child(source)?),
            expression.clone(),
            cached_value.clone(),
        ),
        SnapshotNode::List {
            list_type,
            items,
            sorted,
        } => Datum::List(
            list_type.clone(),
            items.iter().map(child).collect::<Result<_, _>>()?,
            *sorted,
        ),
        SnapshotNode::PropList { entries, sorted } => Datum::PropList(
            entries
                .iter()
                .map(|(key, value)| Ok((child(key)?, child(value)?)))
                .collect::<Result<_, ValueTransferError>>()?,
            *sorted,
        ),
    })
}

fn allocate_destination(
    destination: &mut DirPlayer,
    datum: Datum,
) -> Result<DatumRef, ValueTransferError> {
    destination
        .allocator
        .drain_reclaims(&mut destination.bitmap_manager);
    destination
        .allocator
        .alloc_datum(datum, &mut destination.bitmap_manager)
        .map_err(|error| ValueTransferError::Allocation(error.message))
}

fn rollback_import(destination: &mut DirPlayer, imported: &mut HashMap<SnapshotId, DatumRef>) {
    imported.clear();
    destination
        .allocator
        .drain_reclaims(&mut destination.bitmap_manager);
}

impl OwnedValueSnapshot {
    fn node(&self, id: SnapshotId) -> Result<&SnapshotNode, ValueTransferError> {
        self.nodes.get(id.0).ok_or_else(|| {
            ValueTransferError::SnapshotCorrupt(format!("unknown snapshot node {}", id.0))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::director::lingo::datum::VarRef;
    use crate::player::{
        allocator::ScriptInstanceAllocatorTrait, cast_lib::CastMemberRef, ownership::OwnerToken,
        script::ScriptInstance,
    };
    use async_std::channel;
    use fxhash::FxHashMap;
    use std::collections::VecDeque;

    fn test_player(player: u64) -> DirPlayer {
        let (tx, _rx) = channel::unbounded();
        DirPlayer::new_with_owner(
            tx,
            OwnerToken::new(crate::player::ownership::OwnerKey {
                session: 19,
                player,
                generation: 1,
            }),
        )
    }

    fn target_symbols() -> SymbolTable {
        SymbolTable::with_owner(crate::player::symbols::symbol_table::SymbolOwner {
            session: 19,
            generation: 1,
        })
    }

    #[test]
    fn snapshot_remaps_symbols_and_import_preserves_builtins() {
        let mut source = test_player(1);
        let mut destination = test_player(2);
        let mut source_symbols = target_symbols();
        let mut destination_symbols = target_symbols();
        let dynamic = source_symbols.intern("MiXeD");
        let dynamic_ref = source.alloc_datum(Datum::Symbol(dynamic.clone()));
        let builtin_ref = source.alloc_datum(Datum::Symbol(BuiltInSymbol::True.into()));
        let _different_name = destination_symbols.intern("other");
        let _target_spelling = destination_symbols.intern("mixed");
        let root = source.alloc_datum(Datum::List(
            DatumType::List,
            VecDeque::from([dynamic_ref, builtin_ref]),
            false,
        ));

        let copied = copy_owned_value(
            &source,
            &source_symbols,
            &mut destination,
            &mut destination_symbols,
            &root,
        )
        .unwrap();
        let Datum::List(_, items, _) = destination.get_datum(&copied) else {
            panic!("expected transferred list")
        };
        assert!(matches!(
            destination.get_datum(&items[0]),
            Datum::Symbol(symbol)
                if destination_symbols.display(symbol) == Ok("mixed")
                    && symbol != &dynamic
        ));
        let Datum::Symbol(target_symbol) = destination.get_datum(&items[0]) else {
            panic!("expected remapped dynamic symbol")
        };
        assert!(destination_symbols.display(&dynamic).is_err());
        assert_eq!(source_symbols.display(&dynamic), Ok("MiXeD"));
        assert_eq!(destination_symbols.display(target_symbol), Ok("mixed"));
        assert!(matches!(
            destination.get_datum(&items[1]),
            Datum::Symbol(symbol) if symbol.into_builtin() == Some(BuiltInSymbol::True)
        ));
    }

    #[test]
    fn transfer_preserves_aliases_without_aliasing_source() {
        let mut source = test_player(1);
        let mut destination = test_player(2);
        let source_symbols = target_symbols();
        let mut destination_symbols = target_symbols();
        let child = source.alloc_datum(Datum::String("shared".to_owned()));
        let source_child = child.clone();
        let source_child_id = source_child.unwrap();
        let root = source.alloc_datum(Datum::List(
            DatumType::List,
            VecDeque::from([child.clone(), child]),
            false,
        ));
        let copied = copy_owned_value(
            &source,
            &source_symbols,
            &mut destination,
            &mut destination_symbols,
            &root,
        )
        .unwrap();
        let Datum::List(_, items, _) = destination.get_datum(&copied) else {
            panic!("expected transferred list")
        };
        let target_child = items[0].clone();
        let target_alias = items[1].clone();
        assert_eq!(target_child, target_alias);
        assert_eq!(target_child.unwrap(), source_child_id);
        assert_ne!(target_child, source_child);
        assert!(source.allocator.try_get_datum(&target_child).is_none());
        assert!(destination.allocator.try_get_datum(&source_child).is_none());
        if let Datum::String(value) = destination.get_datum_mut(&target_child) {
            *value = "changed".to_owned();
        } else {
            panic!("expected target string child")
        }
        assert!(matches!(
            destination.get_datum(&target_alias),
            Datum::String(value) if value == "changed"
        ));
        assert!(matches!(
            source.get_datum(&source_child),
            Datum::String(value) if value == "shared"
        ));
        assert_ne!(target_child, root);
    }

    #[test]
    fn transfer_preserves_sorted_proplist_keys_values_and_aliases() {
        let mut source = test_player(1);
        let mut destination = test_player(2);
        let source_symbols = target_symbols();
        let mut destination_symbols = target_symbols();
        let key = source.alloc_datum(Datum::String("key".to_owned()));
        let shared = source.alloc_datum(Datum::String("shared".to_owned()));
        let root = source.alloc_datum(Datum::PropList(
            VecDeque::from([(key.clone(), shared.clone()), (key, shared)]),
            true,
        ));
        let copied = copy_owned_value(
            &source,
            &source_symbols,
            &mut destination,
            &mut destination_symbols,
            &root,
        )
        .unwrap();
        let Datum::PropList(entries, sorted) = destination.get_datum(&copied) else {
            panic!("expected transferred property list")
        };
        assert!(*sorted);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0, entries[1].0);
        assert_eq!(entries[0].1, entries[1].1);
        assert_ne!(entries[0].0, root);
        assert!(matches!(
            destination.get_datum(&entries[0].0),
            Datum::String(value) if value == "key"
        ));
        assert!(matches!(
            destination.get_datum(&entries[0].1),
            Datum::String(value) if value == "shared"
        ));
    }

    #[test]
    fn transfer_preserves_datum_backed_string_chunk_source_and_cache() {
        let mut source = test_player(1);
        let mut destination = test_player(2);
        let source_symbols = target_symbols();
        let mut destination_symbols = target_symbols();
        let source_text = source.alloc_datum(Datum::String("one two".to_owned()));
        let chunk = source.alloc_datum(Datum::StringChunk(
            StringChunkSource::Datum(source_text),
            StringChunkExpr {
                chunk_type: crate::director::lingo::datum::StringChunkType::Word,
                start: 1,
                end: 1,
                item_delimiter: ',',
            },
            "one".to_owned(),
        ));
        let copied = copy_owned_value(
            &source,
            &source_symbols,
            &mut destination,
            &mut destination_symbols,
            &chunk,
        )
        .unwrap();
        let Datum::StringChunk(StringChunkSource::Datum(source_ref), expression, cached) =
            destination.get_datum(&copied)
        else {
            panic!("expected datum-backed string chunk")
        };
        assert!(
            matches!(destination.get_datum(source_ref), Datum::String(value) if value == "one two")
        );
        assert_eq!(expression.start, 1);
        assert_eq!(expression.end, 1);
        assert_eq!(cached, "one");
    }

    #[test]
    fn transfer_rejects_self_and_indirect_cycles_before_destination_mutation() {
        let mut source = test_player(1);
        let mut destination = test_player(2);
        let source_symbols = target_symbols();
        let mut destination_symbols = target_symbols();
        let existing = destination.alloc_datum(Datum::String("keep".to_owned()));
        let cycle = source.alloc_datum(Datum::List(DatumType::List, VecDeque::new(), false));
        if let Datum::List(_, items, _) = source.get_datum_mut(&cycle) {
            items.push_back(cycle.clone());
        }
        let before = destination.allocator.datum_count();
        assert!(matches!(
            copy_owned_value(
                &source,
                &source_symbols,
                &mut destination,
                &mut destination_symbols,
                &cycle,
            ),
            Err(ValueTransferError::CyclicDatum)
        ));
        assert_eq!(destination.allocator.datum_count(), before);

        let mut foreign_symbols = target_symbols();
        let foreign_symbol = foreign_symbols.intern("foreign-symbol");
        let foreign_symbol_ref = source.alloc_datum(Datum::Symbol(foreign_symbol));
        assert!(matches!(
            copy_owned_value(
                &source,
                &source_symbols,
                &mut destination,
                &mut destination_symbols,
                &foreign_symbol_ref,
            ),
            Err(ValueTransferError::InvalidReference(_))
        ));
        assert_eq!(destination.allocator.datum_count(), before);
        assert!(matches!(
            destination.get_datum(&existing),
            Datum::String(value) if value == "keep"
        ));

        let first = source.alloc_datum(Datum::List(DatumType::List, VecDeque::new(), false));
        let second = source.alloc_datum(Datum::List(DatumType::List, VecDeque::new(), false));
        if let Datum::List(_, items, _) = source.get_datum_mut(&first) {
            items.push_back(second.clone());
        }
        if let Datum::List(_, items, _) = source.get_datum_mut(&second) {
            items.push_back(first.clone());
        }
        assert!(matches!(
            copy_owned_value(
                &source,
                &source_symbols,
                &mut destination,
                &mut destination_symbols,
                &first,
            ),
            Err(ValueTransferError::CyclicDatum)
        ));
    }

    #[test]
    fn transfer_handles_deep_acyclic_dags_iteratively() {
        let mut source = test_player(1);
        let mut destination = test_player(2);
        let source_symbols = target_symbols();
        let mut destination_symbols = target_symbols();
        let mut current = source.alloc_datum(Datum::String("leaf".to_owned()));
        for _ in 0..1024 {
            current = source.alloc_datum(Datum::List(
                DatumType::List,
                VecDeque::from([current]),
                false,
            ));
        }
        let copied = copy_owned_value(
            &source,
            &source_symbols,
            &mut destination,
            &mut destination_symbols,
            &current,
        )
        .unwrap();
        let mut cursor = copied;
        for _ in 0..1024 {
            let Datum::List(_, items, _) = destination.get_datum(&cursor) else {
                panic!("deep transfer lost a list node")
            };
            cursor = items[0].clone();
        }
        assert!(matches!(
            destination.get_datum(&cursor),
            Datum::String(value) if value == "leaf"
        ));
    }

    #[test]
    fn transfer_rejects_foreign_nested_and_unsupported_refs_before_mutation() {
        let mut source = test_player(1);
        let mut foreign = test_player(2);
        let mut destination = test_player(3);
        let source_symbols = target_symbols();
        let mut destination_symbols = target_symbols();
        let foreign_instance = foreign.allocator.alloc_script_instance(ScriptInstance {
            instance_id: 1,
            script: CastMemberRef {
                cast_lib: 1,
                cast_member: 1,
            },
            ancestor: None,
            properties: FxHashMap::default(),
            begin_sprite_called: false,
        });
        let foreign_ref = source.alloc_datum(Datum::ScriptInstanceRef(foreign_instance));
        let nested = source.alloc_datum(Datum::List(
            DatumType::List,
            VecDeque::from([foreign_ref]),
            false,
        ));
        let before = destination.allocator.datum_count();
        assert!(matches!(
            copy_owned_value(
                &source,
                &source_symbols,
                &mut destination,
                &mut destination_symbols,
                &nested,
            ),
            Err(ValueTransferError::InvalidReference(_))
        ));
        assert_eq!(destination.allocator.datum_count(), before);

        let unsupported = source.alloc_datum(Datum::VarRef(VarRef::Script(CastMemberRef {
            cast_lib: 1,
            cast_member: 1,
        })));
        assert!(matches!(
            snapshot_owned_value(&source, &source_symbols, &unsupported),
            Err(ValueTransferError::UnsupportedDatum(DatumType::VarRef))
        ));

        let stage = source.alloc_datum(Datum::Stage);
        assert!(matches!(
            snapshot_owned_value(&source, &source_symbols, &stage),
            Err(ValueTransferError::UnsupportedDatum(DatumType::StageRef))
        ));
        let flash = source.alloc_datum(Datum::FlashObjectRef(
            crate::director::lingo::datum::FlashObjectRef::from_path("unsupported"),
        ));
        assert!(matches!(
            snapshot_owned_value(&source, &source_symbols, &flash),
            Err(ValueTransferError::UnsupportedDatum(
                DatumType::FlashObjectRef
            ))
        ));
    }

    #[test]
    fn snapshot_survives_source_reset_and_destination_owner_is_checked() {
        let mut source = test_player(1);
        let mut destination = test_player(2);
        let source_symbols = target_symbols();
        let mut destination_symbols = target_symbols();
        let root = source.alloc_datum(Datum::String("snapshot".to_owned()));
        let snapshot = snapshot_owned_value(&source, &source_symbols, &root).unwrap();
        source.reset_owned_core();
        assert!(matches!(
            snapshot_owned_value(&source, &source_symbols, &root),
            Err(ValueTransferError::InvalidReference(_))
        ));
        let copied =
            import_owned_value(&snapshot, &mut destination, &mut destination_symbols).unwrap();
        assert!(
            matches!(destination.get_datum(&copied), Datum::String(value) if value == "snapshot")
        );

        let dead_owner = destination.owner.clone();
        dead_owner.begin_reset();
        assert!(matches!(
            import_owned_value(&snapshot, &mut destination, &mut destination_symbols),
            Err(ValueTransferError::DestinationOwnerDead)
        ));
    }
}
