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

An explicit value-transfer module is being authored separately with source preflight, owned graph snapshots, symbol remapping, alias preservation, iterative traversal and typed cycle rejection. It remains unintegrated and unverified. Existing `duplicate` supports bitmap and object cases outside the transferable-value allowlist, so replacing it wholesale with the new API would be a compatibility regression. Its cycle repair must preserve those existing cases and receive a dedicated regression before Stage 2.1 acceptance.
