# R1 fresh-Player bootstrap receipt

Date: 2026-09-22

Status: **accepted prerequisite**. R1 constructs an isolated, source-free Ruffle
candidate and resolves the actual frame-371 AVM1 native builtins into that
candidate. It does not restore mutable graph state, display/timeline resources,
continuation, a fresh worker, Director audio, or C1.

## Pinned inputs and publication

| Item | Identity |
| --- | --- |
| Parent baseline | `febb0dd0bff8d18ffb6c961ec4c89ce2b85898fc` on `save-state` |
| Ruffle baseline | `83c6d65f27a861e7adad3e56837a4fa481ae17fe` |
| Accepted Ruffle commit | `13c16ae1f65bf27537d4581f9f4ab556b03f5c34` on `iExalt/ruffle:r1-fresh-player-bootstrap` |
| gc-arena dependency | `https://github.com/iExalt/gc-arena`, commit `682dc66ff12738cd8f3c3c8f268f10c203366240` |
| Asset | `opening_anim.swf`, SHA-256 `d964a7e594109f8923004333e129f9c655fabde0533a4b4bf74dce238bf3215f` |
| Tooling | lane `mise.toml`, Rust 1.98.1, Cargo `--locked --offline` |

## Accepted behavior

[`CheckpointCandidate`](../../../../ruffle/core/src/checkpoint_bootstrap.rs)
owns a separately built Ruffle `Player`, GC arena, null host backends, memory
storage, and execution guard. It copies the qualified source player's exact
player version and runtime setting, supplies no movie, disables autoplay and
the optional default font, and publishes no candidate into the source player.

Construction verifies no root movie or current frame, idle frame phase, empty
action and post-frame queues, an empty AVM1 persistent stack, four undefined
AVM1 persistent registers, empty AVM2 operand/scope stacks, and an empty AVM2
call stack. Guard counters remain zero for `run_frame`, `goto_frame`, preload,
action-queue drains, action execution, and post-frame callbacks. Ruffle's
intrinsic playerglobal and empty-stage bootstrap still runs; R1 proves absence
of movie-source/frame/action execution, not absence of intrinsic bootstrap.

The actual frame-371 census projected 1,854 pointer-free logical bindings:
1,342 `Native` and 512 `TableNative`. The candidate resolved all 1,854 from
case-qualified persistent bootstrap roots and stored properties. Getter and
setter function slots are inspected without invoking them; prototype lookup
and normal ActionScript property access are not used. Raw object/function
addresses never enter the DTO or receipt. Arena-local pointer equality is used
only to reject identity conflicts.

Resolution is atomic and one-shot. Exact duplicate graph IDs reuse one logical
handle; conflicting paths, descriptors, graph IDs, or target objects reject the
batch before slot publication. Inputs above 10,000 bindings fail before player
locking or arena traversal. Successful and failed private candidates release
their retained ownership token on drop. The source lock is released before
candidate construction, and the complete pointer-free source census receipt,
frame 371, and callback FIFO compare equal before and after the candidate run.

## Commands and results

```text
CARGO_TARGET_DIR=$PWD/.cache/r1-bootstrap-target-r1 mise exec -- cargo test -p ruffle_core --locked --offline checkpoint_bootstrap -- --nocapture
```

Result: six passed, zero failed. Coverage includes representative native and
table-native resolution, alias reuse and identity isolation, collision and
descriptor rejection, the 10,000/10,001 input boundary, atomic retry, and
failed-candidate cleanup.

```text
PARITY_OPENING_ANIM_SWF=/Users/clliaw/Projects/childhood-redux/.cache/spybot-recovery/embedded-probe/opening_anim.swf CARGO_TARGET_DIR=$PWD/.cache/r1-bootstrap-target-r1 mise exec -- cargo test --manifest-path vm-rust/Cargo.toml --locked --offline native_flash_checkpoint_bootstrap_resolves_actual_frame_371_bindings -- --ignored --nocapture
```

Result: one passed, zero failed. Targeted `rustfmt --check` and parent/child
`git diff --check` also passed. Existing repository warnings were unchanged.

## Boundary for R2

R1 handles contain candidate-local logical graph IDs and slots, but do not yet
retain arena `Gc` objects. R2 must add candidate-owned rooted storage and a
two-pass allocate/fixup operation for the supported mutable AVM1 subset. It must
restore properties, attributes, prototypes, aliases, and cycles without raw
pointer export, source execution, getters, fake display placeholders, or
dynamic-root leaks. Display/timeline identity, other resources, continuation,
fresh-worker consumption, and Director audio remain later components.

R2 was accepted on 2026-09-22 as a bounded prerequisite; see the
[R2 AVM1 graph receipt](../../save-state-r2-20260922/graph/R2_AVM1_GRAPH_RECEIPT.md).
It round-trips an actual 16-node subset that is mostly builtin/bootstrap state,
plus synthetic fresh plain-object cases. It does not close the wider boundary
described above: arrays, accessors, MovieClips, display/timeline/resources,
continuation, fresh-worker consumption, and Director audio remain deferred.
