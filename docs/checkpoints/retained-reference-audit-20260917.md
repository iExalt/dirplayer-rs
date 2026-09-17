# Retained reference audit — 2026-09-17

The initial source review below began at `7d9ca56`. The final acceptance section records Stage 2.1 closure against `d8f868fd` plus three allocator regressions, with a fresh 573-test native result. Earlier pending statements describe their historical checkpoints.

| Kind | Authority and consumer boundary | Finding |
| --- | --- | --- |
| `DatumRef` | Allocator lookup validates the captured owner and allocation identity before returning the value. | Local payload coordinates do not bypass the outer capability. |
| `MovieRef` | Unit marker produced by `_movie`/`_system`; property access resolves against the explicit current execution context. | No foreign arena pointer is embedded. |
| `SpriteRef(i16)` | Local score coordinate; retained value is enclosed in an owned `DatumRef`. | A same-number sprite in a different player is not authority for a foreign datum. |
| `ScriptInstanceRef` / `VarRef::ScriptInstance` | Allocator `valid_script_ref` checks owner identity/liveness, ID and reference-count identity. Common graph validation checks direct and variable-reference forms. | No additional unowned handle found. |
| Lists, property lists, datum-backed string chunks, timeout payloads | `validate_owned_datum_graph` traverses retained children iteratively. Each arena lookup precedes numeric-ID cycle deduplication. | Foreign colliding child IDs cannot skip ownership validation. |
| `CastMemberRef` / `VarRef::Script` | Coordinates resolved within the owning movie; no independent retained arena authority. | No cross-player transfer authority found in this scoped review. |
| `BitmapHandle` | Exact owner/reference identity checked by the bitmap manager. | Accepted bitmap checkpoint remains the test evidence. |
| `JsObjectHandle` | Opaque owner capability plus registry lookup; explicit get/set/call validates before host work and before result conversion. | Accepted JS checkpoint and consumer audit cover this kind. |
| `FlashObjectRef` | Host operations require an owner key, local sprite binding and instance generation via `owned_binding`. Owned decoded responses attach these fields. | Initially unbound raw-conversion values fail closed at host access; initial-access lifecycle remains a separate Stage 2.6 item. |

Independent review traced producers and consumers in `bytecode/get_set.rs`, `handlers/movie.rs`, `eval.rs`, `script.rs`, `allocator.rs`, `driver.rs` and the Flash datum handler. It found no justified new capability wrapper or compatibility expansion for the local-coordinate variants above. The navigator additionally inspected the graph validator and explicit JS registry boundary.

Remaining Stage 2.1 work includes completing the other retained-kind inventory and validating explicit-copy semantics, recursive reclamation, scope invalidation and owner-collision coverage. A source-level absence of a defect does not substitute for those exit tests. No complete Stage 2.1 acceptance is claimed.

## Verified existing test receipts

The saved final JS-checkpoint library log reports exit 0 and the following executed tests. These are evidence for accepted revision `7d9ca56`; they do not validate the in-progress Flash patch. No tests were rerun for this audit.

```text
test player::allocator::ownership_tests::deferred_reclaim_reaches_children ... ok
test player::allocator::ownership_tests::nested_list_and_proplist_reclaim_bitmap_once ... ok
test player::bitmap::manager::tests::copy_requires_source_capability_and_creates_independent_bitmap ... ok
test player::scope_token_tests::owner_generation_and_epoch_are_required_for_scope_access ... ok
test result: ok. 536 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.53s
```

Raw receipt: `/private/tmp/dirplayer-js-object-validation-20260917/final-freeze-2/native-suite.stdout` and `native-suite.exit`. The JS checkpoint preserves the binary identity, source manifest and command. The navigator also read the scope test: it rejects a token from a distinct owner with identical numeric keys, rejects invalidated epochs and reused scope generations, and keeps the neighboring owner usable.

## Copying findings and resolved components

The original follow-up audit found no general cross-owner Datum-copy API and an uncontrolled-recursion path in same-owner `player_duplicate_datum_inner`. Those findings describe the earlier checkpoint, not the current implementation: value transfer was introduced in `4e103fdf`, and `243647a3` replaces recursive duplication with iterative preflight and reconstruction.

The explicit `player::value_transfer` module provides source preflight, owned graph snapshots, symbol remapping, alias preservation, iterative traversal and typed cycle rejection. Its eight focused native tests passed. Existing `duplicate` supports bitmap and object cases outside the transferable-value allowlist, so it retains separate semantics: iterative per-occurrence copying preserves ordinary leaf clones and independent bitmap storage. Six focused duplication tests and the 554-test native suite pass; the [duplication receipt](iterative-duplication-20260917/README.md) records source identity and verification limits. These accepted components do not close the remaining reference-consumer, reclamation and scope gates.


## Value-transfer component evidence

The focused receipt at `/private/tmp/dirplayer-stage2-6-validation/value-transfer/run.stdout`
reports eight passed, zero failed; `run.exit` records zero. Source
`vm-rust/src/player/value_transfer.rs` has SHA-256
`90b80dd2842a1fbda0b2886ed97b29fe69fecaf815f7140f56d8566a2ce40784`.
Coverage includes destination symbol remapping, builtins, independent imported
storage, DAG aliases, sorted property-list keys and values, datum-backed string
chunks, self/indirect cycles, deep acyclic graphs, foreign nested capabilities,
unsupported references, source reset after snapshot and destination owner death.

Snapshot preflight rejects invalid source graphs before destination mutation.
Import does not promise rollback of symbol interning or OOM/panic recovery.
Bitmap/object copying keeps its separate existing semantics; the same-owner
duplication repair and its evidence are described above.


## Complete Datum payload shape inventory

A follow-up read of the complete `Datum` enum at checkpoint `4e103fdf` adds the
following shapes to the earlier retained-capability table. This is a payload
inventory, not proof that every manager consumer has completed its owner cutover.

| Remaining variants | Retained data and required boundary |
| --- | --- |
| Int, Float, String, Void, Null, Rect, Point, ColorRef, Vector, Transform3d, JavaScript | Owned scalar/byte/value payloads; no nested arena capability. |
| CastLib, Stage, SoundRef, SoundChannel, CursorRef, TimeoutRef, TimeoutFactory, Xtra, XtraInstance, PlayerRef, MouseRef, XmlRef, DateRef, MathRef | Markers, names or local IDs. The enclosing DatumRef is owned; the target manager must resolve them from the explicit execution context. Manager completion remains Stage 2.2/2.5/2.9 work. |
| PaletteRef | Builtin/default palette or local CastMemberRef coordinates; palette consumers still require owner-local movie resolution. |
| Matte | Arc of owned BitmapMask width/height/bit data; no DatumRef or interior mutable cell in the payload. |
| Media | Owned Field, Bitmap, Palette or Sound payload. Bitmap embeds a palette coordinate and optional shared mask; its palette meaning remains movie-local. General value transfer intentionally rejects Media. |
| Shockwave3dObjectRef, HavokObjectRef, PhysXObjectRef | Member coordinates, builtin object type, local ID where applicable, and a Symbol name. `validate_direct_symbol_fields` validates those name symbols; scene lookup must use the explicit player's member. |
| VectorVertexRef | Local member coordinate plus vertex index. Owner authority comes from its enclosing DatumRef and explicit member lookup. |

The graph validator traverses List, PropList, datum-backed StringChunk and
TimeoutInstance children. It separately validates ScriptInstanceRef and
BitmapHandle leaves. JS and Flash object capabilities have additional checks at
their host-operation boundary; the generic graph validator alone is not proof
of their host liveness. This distinction must be retained in the final Stage 2
acceptance audit.

## Remaining reclamation verification

At `243647a3`, allocator reclamation removes a datum and drains any child-drop
records until its owner-local queue is empty. `StringChunkSource::Datum` retains
one datum reference; `TimeoutInstanceData` retains callback, target and optional
script-instance datum references. The existing driver fixtures exercise foreign
references in these payloads, but the allocator's recursive reclamation fixture
uses a List. Dedicated chunk/timeout reclamation evidence is still missing.

The next scoped tests should drop those parent values, drain the real allocator
queue, and verify unshared descendants are reclaimed while externally retained
children survive. Repeat across reset and colliding local IDs in a neighboring
owner to prove old drops cannot reclaim replacement or foreign allocations.
This records a verification gap, not a newly demonstrated implementation defect.

## Stage 2.1 closure review

Three additional allocator regressions are preserved as uncommitted work and excluded from the accepted publication:
`string_chunk_reclaims_unshared_source_but_preserves_retained_child`,
`timeout_instance_reclaims_owned_children_but_preserves_retained_script_datum`,
and `stale_chunk_timeout_drops_cannot_reclaim_replacements_or_neighbor_owner`.
Allocator SHA-256 after integration is
`91eb904b5a1cefcf61aff06f6bee6b1817dcb905249cd413cf6f2568d3eefd7b`.
They are not in the accepted 570-test artifact or the published source and remain unverified.

The remaining Stage 2.1 evidence map is concrete:

| Requirement | Source boundary and existing evidence |
| --- | --- |
| Datum/script capability rejection | Allocator validates exact owner, liveness, arena ID and refcount identity before returning entries; existing allocator/driver foreign, stale and collision fixtures pass. |
| Nested retained values | Graph validation checks each reference before numeric-ID deduplication and traverses lists, property lists, datum-backed chunks and timeout fields. Script and bitmap leaves have separate capability checks. |
| Symbols and bitmaps | Symbol-table display validates direct and embedded symbol ownership; accepted bitmap/typed-mutation checkpoints cover read, mutation, replacement, refcount and reset collisions. |
| Explicit copying | Accepted value transfer and iterative duplication retain their distinct contracts; eight and six focused tests respectively pass, including aliases, cycles, symbols, unsupported resources and bitmap copying. |
| Scope invalidation | ScopeToken checks exact owner, liveness, player epoch, slot and scope generation; existing scope owner/epoch/replacement tests pass. |
| Recursive reclamation | Existing list/property-list/bitmap/script/reset fixtures pass. The three local, unpublished chunk/timeout tests above must still execute. |

The navigator independently checked allocator script-reference validation,
graph traversal, script property access and scope validation. Script-instance
property maps are not recursively traversed by the graph validator; production
property reads validate returned datum references before dereferencing them.
Local-coordinate variants carry no independent arena capability. Remaining
manager/host routing migrations belong to the explicit interpreter, extension,
Flash, JS and residual-service gates, and are not declared complete by this audit.

No additional retained-capability defect was identified in this scoped review.
Stage 2.1 acceptance remains pending execution of the new tests against a recorded
coherent checkout; source review alone does not close it.

## Stage 2.1 acceptance

The three reclamation regressions now pass individually and in the fresh
**573 passed, 0 failed** native suite. The tested source is an isolated export
of published `d8f868fd` plus only those tests; its allocator hash matches the live
file. All 419 input-file hashes were checked after execution.
[The portable receipt](retained-reclamation-20260917/README.md) retains exact
source, artifact identity, commands and test output.

Combined with the reviewed payload/consumer map above, the suite establishes
foreign/stale arena rejection, explicit-copy semantics, recursive reclamation
and scope invalidation with owner/ID collision coverage. Stage 2.1 is accepted
at this source identity. The three regressions are included in this publication. This does not certify
the concurrent child Flash implementation or close the separate host/manager,
interpreter and final ownership-cutover requirements.
