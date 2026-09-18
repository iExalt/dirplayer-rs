# Accepted Stage 2.7 callback-origin component

Accepted locally at the source and artifact identities in [acceptance.json](acceptance.json).
Full Stage 2 remains open. These working-tree changes are unpublished.
The [preceding component](../score-child-pointer-20260917/README.md) remains a
separate acceptance boundary.

## Behavior

Lingo callback registration captures owner, sprite and generation before host
work, including initial publication and already-ready instances. Ruffle keeps
its registry per instance; direct and bridge transport carry the captured origin
through the stable frontend route. VM dequeue rejects stale callbacks before
JSON/base64 decoding or allocation. Retained object markers keep that same
binding recursively.

The actual authored AVM1 fixture verifies two owners with overlapping sprite IDs,
Unicode/numeric/object arguments, ready-cache registration, retained-object
property access and staleness, queued-valid callbacks invalidated before dequeue,
reset/recreation, actual handle destruction, rejected disposed callbacks and a
surviving sibling's fifth callback.

Necessary integration repairs also cover explicit-owned sprite.callFunction,
typed SpriteAsync evaluator completion after script override lookup, synchronous
identity admission before future construction, and continued pumping of ready
cooperative work. Sprite call argument-string and String-or-Void semantics are
preserved. Started external waits remain parked. Temporary diagnostic counters,
markers, hard caps and generated runner overrides are removed.

## Verification

| Gate | Result |
| --- | --- |
| Native suite | 612 passed, 0 failed |
| Formerly failing MovieAsync cooperative pump | 1 passed |
| Identity admission, cancellation and nested action regression | 1 passed |
| Frontend manager | 12 passed |
| Frontend lifecycle/LocalConnection | 38 passed |
| Focused actual callback and disposal fixture | 1 passed |
| Combined actual browser selection | 20 passed, 0 failed |
| Production VM package and frontend build | Passed |

The navigator inspected the final receipts and critical lifecycle/ownership
interfaces, checked all 42 final runtime-source hashes, verified untouched
sources inherited from the preceding acceptance, and verified evidence/artifact
checksums. Generated package bindings expose the new callback API; the built
frontend WASM exactly matches the package. Production raw WASM, processed package
WASM, browser-test WASM and Ruffle WASM have separate recorded identities.
The 61-file accepted source overlay is archived under
`~/Projects/.dirplayer-work/stage2-7/accepted-source-overlay.tar.gz`; its hash and
base revisions are recorded in acceptance.json so later edits do not erase this
working-tree source identity.

Receipts remain under `~/Projects/.dirplayer-work/stage2-7/validation`.
Final manifests are `c16-final-runtime-source-freeze3.sha256`,
`c16-final-artifacts.sha256` and `c16-final-evidence.sha256`. The final combined
receipt is `c16-browser-combined20-final2.log`; production package and frontend
receipts are `c16-wasm-pack-production-final2.log` and
`c16-frontend-production-build2.log`. Pinned package generation used wasm-pack
0.15.0 and wasm-bindgen 0.2.108 with locked/offline Cargo.

The combined run uses the Stage 2.7 browser runner and the shared native-stage2
Cargo target under `~/Projects/.dirplayer-work`, one Playwright worker and a
180000 ms timeout. Its filter adds `test_flash_lingo_callback_owned_fixture` to
the preceding 19-fixture ownership selection. The receipt records actual test
names; Playwright reports one outer suite containing 20 VM fixtures.

## Limits and follow-up

Transport tests do not qualify an actual MV3 extension runtime. LocalConnection
has native/frontend regressions, not a dedicated real-browser fixture in this
selection. The bridge object-get return gap and adjacent sprite get/set methods
remain open; see [the next source audit](../sprite-variable-ownership-audit-20260918.md).
General unsupported Object requeue, broader async cancellation, input/timers,
services and final ownership cutover remain separate Stage 2 work.

The [historical record](implementation-history.md) retains superseded failures
and diagnostic decisions. Final green receipts supersede the native 611/1
scheduler failure, early playback observation failure and WASM helper compile
failures; none are concealed or treated as acceptance evidence.
