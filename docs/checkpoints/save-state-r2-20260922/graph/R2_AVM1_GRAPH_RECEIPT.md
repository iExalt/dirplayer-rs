# R2 bounded AVM1 graph receipt

Date: 2026-09-22

Status: **accepted prerequisite**. R2 captures and restores a pointer-free,
actual 16-node frame-371 AVM1 subset in the private R1 candidate with exact
normalized values, attributes, prototypes, aliases, and cycles. The selected
subset is mostly bootstrap/builtin state and proves no game continuation: it
contains two bootstrap-reused objects and fourteen native functions, with no
fresh allocation. Display/timeline/resources, arrays, accessors, continuation,
a fresh worker, Director audio, and C1 remain unimplemented.

## Pinned inputs and publication

| Item | Identity |
| --- | --- |
| Parent baseline | `dca71e68d9feb833e1e74a18d1e24ea57df19e4b` on `save-state` |
| Ruffle baseline | `13c16ae1f65bf27537d4581f9f4ab556b03f5c34` |
| Accepted Ruffle commit | `979ca0ef98769a29f8d7b466a2133d45780afa9d` on `iExalt/ruffle:r2-avm1-graph` |
| gc-arena dependency | `https://github.com/iExalt/gc-arena`, commit `682dc66ff12738cd8f3c3c8f268f10c203366240` |
| Asset | `opening_anim.swf`, SHA-256 `d964a7e594109f8923004333e129f9c655fabde0533a4b4bf74dce238bf3215f` |
| Tooling | lane `mise.toml`, Rust 1.98.1, Cargo `--locked --offline` |

## Accepted behavior

[`checkpoint_graph.rs`](../../../../ruffle/core/src/checkpoint_graph.rs) defines
bounded pointer-free graph IDs and DTOs. Strings retain their exact narrow or
wide code units; numbers retain their exact bits; stored-property insertion
order and all attribute bits are preserved. Capture rejects getters/setters,
interfaces, watchers, unknown object kinds, duplicate property names under
Flash string equality, invalid references, and the 10,000-node/50,000-strong-
edge/10,000-weak-edge limits.

The candidate owns traced `GraphId` to AVM1-object storage. Restore first
allocates supported fresh plain/script objects or resolves the R1 logical
bootstrap/native binding, then fixes stored properties and prototypes. Raw
addresses do not enter the DTO. Identity, aliases, and cycles are checked with
arena-local identity only. The original player is read-only, source getters and
scripts are not invoked, and display objects are not replaced with placeholders.

Restore normalizes the candidate back to the exact DTO before publication.
Invalid roots, fixup errors, normalization differences, and publication errors
poison and drop the private candidate. The source player remains unchanged. A
lock-lifetime defect found by the focused root-binding failure test was fixed by
ending each player-lock scope before cleanup reacquires the candidate; the
failure and retry paths now complete without deadlock.

## Actual-profile result

The complete AVM1 census contains 2,023 nodes and 4,199 strong edges: 148
`None`, 1,854 `Function`, 16 `Array`, and five `MovieClip` nodes; edges comprise
3,351 data properties, 518 getters, and 330 setters. All 148 `None` nodes classify
as bootstrap reuse with digest
`a11768b4d53750c80ba12e3192705d76571b8713c162daaef06040971710c7f2`.

The accepted actual closure starts at graph root 2 and contains 16 nodes, 31
data edges, no accessors, no weak edges, and no scalar strings. It has alias
targets 2 and 4 and six back edges. The fresh candidate resolved the two
bootstrap objects and fourteen native functions, restored the closure, and
produced an exact normalized DTO. The source census, frame 371, callback FIFO,
and source-free execution guard were unchanged; the candidate interner was also
unchanged.

This leaves 2,007 actual nodes and 4,168 actual edges deferred: 146 `None`,
1,840 `Function`, 16 `Array`, five `MovieClip`, 3,320 data edges, 518 getters,
and 330 setters. Interner weak retention, actions, timers, clocks, callbacks,
render state, display/timeline/resources, and continued execution are not
restored by R2.

## Commands and results

```text
CARGO_TARGET_DIR=$PWD/.cache/r2-graph-target mise exec -- cargo test -p ruffle_core --locked --offline checkpoint_graph -- --nocapture
```

Result: nine passed, zero failed. Coverage includes fresh plain-object allocation,
exact scalar/property/attribute state, prototype/alias/cycle fixup, invalid and
conflicting references, bounds, normalized mismatch cleanup, root-binding
mismatch cleanup, and retry after atomic failure.

```text
PARITY_OPENING_ANIM_SWF=/Users/clliaw/Projects/childhood-redux/.cache/spybot-recovery/embedded-probe/opening_anim.swf CARGO_TARGET_DIR=$PWD/.cache/r2-graph-target mise exec -- cargo test --locked --offline --lib native_flash::tests::native_flash_checkpoint_bootstrap_resolves_actual_frame_371_bindings -- --ignored --nocapture
```

Result after the final lock-scope correction: one passed, zero failed, with 766
tests filtered. It reproduced the 16-node/31-edge closure, exact normalized DTO,
all 148 bootstrap classifications and digest, zero guard calls, and unchanged
source census/frame/callback data. Targeted `rustfmt --check` and parent/child
`git diff --check` passed; existing warnings were unchanged.

## Boundary for R3

R3 must capture and privately reconstruct the actual 11-node display tree: the
five MovieClips plus Stage/Graphics identity, exact parent/depth/mask/AVM
crosslinks, and every canonical per-node field required for later timeline and
resource restoration. It must bind pinned-asset definitions and link the exact
AVM object IDs before fixup without `run_frame`, `goto_frame`, movie scripts, or
getters. Unrepresented timeline or render fields reject explicitly. Arrays and
accessors remain a later AVM expansion component; full continuation and all
actual relevant nodes remain requirements of the larger fresh-Player experiment.
