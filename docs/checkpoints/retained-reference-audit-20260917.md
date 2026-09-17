# Retained reference audit — 2026-09-17

Scoped source review of the accepted `7d9ca56` ownership implementation, supplemented by the bitmap and JS checkpoint test receipts. This records reviewed reference kinds; it does not close Stage 2.1 or add a new test result.

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

## Concrete remaining copying work

The follow-up source audit found no general cross-owner Datum-copy API; existing explicit inter-player copying is bitmap-specific. Same-owner `player_duplicate_datum_inner` recursively walks Lists/PropLists after the iterative graph validator accepts them. A self-referential List can be constructed by the existing `driver.rs` graph-validation fixture, so passing that graph to duplication has no termination condition. This is a source-proven uncontrolled-recursion path; the audit did not execute an intentional process-crashing test.

The explicit `player::value_transfer` module is integrated in checkpoint `4e103fdf`, with source preflight, owned graph snapshots, symbol remapping, alias preservation, iterative traversal and typed cycle rejection. Its eight focused native tests passed against the compiler-4 artifact; the later compiler-5 full suite also passed these tests but failed an unrelated Flash fixture (543 passed, one failed overall). This verifies the bounded value-transfer component, not the complete Stage 2.1 gate. Existing `duplicate` supports bitmap and object cases outside the transferable-value allowlist, so replacing it wholesale with the new API would be a compatibility regression. Its cycle repair must preserve those existing cases and receive a dedicated regression before Stage 2.1 acceptance.


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
recursive `duplicate` path still requires the repair described above.


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
