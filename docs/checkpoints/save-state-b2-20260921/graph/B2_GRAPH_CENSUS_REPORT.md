# B2 Ruffle graph census

Status: **display-bound AVM1 slice passed; full graph remains blocked at explicit incomplete coverage**.

This B2 item is a diagnostic census and continuation check. It does not implement a codec, restore, fresh-player rehydration, durable format, Director audio codec, or production framework. The parent lane is `save-state` at baseline `480430efec6242bf2aaab52e2135db11d35838b6`; its lane-local Ruffle branch is `b2-graph-census` at pinned base revision `fd5d8dda1cc7b8cf91de141a48574750fa8f86b2`. The child changes were published as commit `6ebf1bcf7c5d26deb83ee4a2b2b8c318b07de3de` to the trusted fork `https://github.com/iExalt/ruffle.git` on that branch. The checkout's local origin remains `https://github.com/chameleonxxl/ruffle.git`; it was preserved and not used after auto-review rejected it. Child commit/push are complete. Parent commit/push fields in the receipt describe the pre-publication evidence-review point because the parent commit cannot embed its own eventual hash.

## Representative result

The focused test loaded the accepted read-only [`opening_anim` asset inventory](../../save-state-b1-20260921/assets/SPYBOT_ASSET_INVENTORY.md) (SWF SHA-256 `d964a7e594109f8923004333e129f9c655fabde0533a4b4bf74dce238bf3215f`; DCR SHA-256 `ddf24b667a8d014856d9db42e1658cbf9714847f1cadc2c2c14d21f8e5950c77`), reached frame 371, and captured the census while the player was `FramePhase::Idle`. The census uses `Player::enter_arena` and does not call update, `run_frame`, render, tick, GC, source execution, or callback draining. The frame and callback FIFO depth were unchanged by the census.

The census reports all 23 `GcRootData` fields from `ruffle/core/src/player.rs:146-207`, each with `Present`, `Empty`, or `Unknown` occupancy and an explicit implemented flag. The names are: `library`, `stage`, `mouse_data`, `drag_object`, `avm1`, `avm2`, `action_queue`, `interner`, `load_manager`, `avm1_shared_objects`, `avm2_shared_objects`, `unbound_text_fields`, `timers`, `current_context_menu`, `external_interface`, `audio_manager`, `stream_manager`, `sockets`, `net_connections`, `local_connections`, `orphan_manager`, `dynamic_root`, and `post_frame_callbacks`. It reports eight named `Library` fields total from `ruffle/core/src/library.rs:429-457`: `movie_libraries`, `device_fonts`, `global_fonts`, `font_lookup_cache`, `font_sort_cache`, `default_font_names`, `default_font_cache`, and `avm2_class_registry`. All eight traversal/classification surfaces are incomplete; `movie_libraries` has observed `Present` occupancy and the remaining seven fields have `Unknown` occupancy.

The observed opening/title boundary had AVM1 stack length zero and four `Undefined` persistent register values, host callback FIFO depth zero, and no reported live AVM2 execution. AVM2 operand-stack length and execution state remain `Unknown` because the pinned public read-only surface does not expose the needed status. These are explicit incomplete fields, not inferred emptiness. Strong-node, edge, and weak-edge counts are also unset. The census therefore reports `coverage_complete=false` and `incomplete_coverage`.

The retained runtime `unsupported` output is exactly `incomplete_coverage`, `avm2_operand_stack_not_inspected`, and `avm2_execution_state_incomplete`. The separate source-review blockers below describe missing private inspection surfaces; they are not additional runtime observations from this test.

The approved display-bound AVM1 slice then walked the existing Stage/display tree without allocating AVM1 objects, traversing general globals or properties, executing source, or touching a second subsystem. It observed 11 display records in render-list order: Stage with no AVM1 object; MovieClip with `MovieClip` objects at ordinals 1, 2, 3, 5, and 6; Graphic with no AVM1 object at ordinals 4, 7, 8, 9, and 10. Every encountered object was either a recognized display-native MovieClip or an absent AVM1 binding; no native function, opaque native kind, or unknown display-bound kind occurred. The bounded traversal counted 11 nodes and 10 edges against 10,000-node and 50,000-edge caps and stopped with no budget reason. The child adapter returns a stop signal on the first rejected edge, so a cap hit terminates render-list enumeration immediately while preserving render-list order and avoiding a materialized child vector. This slice is `eligible_for_next_graph_step`: that label means only that this bounded slice had no observed blocker. It is not restore eligibility and does not establish complete graph coverage.

The slice uses `DisplayObject::checkpoint_existing_avm1_object` and `checkpoint_for_each_child` at `ruffle/core/src/display_object.rs:2912-2927`, the exhaustive native-kind map at `ruffle/core/src/avm1/object/script_object.rs:838-876`, and function-kind inspection at `ruffle/core/src/avm1/function.rs:467-474`. The bounded traversal, display-kind classification, and nonallocating existing-object path are in `ruffle/core/src/player.rs:480-619`; the focused assertions are in `vm-rust/src/native_flash.rs:1179-1251`.

The census player and a matched no-census control were both built from the same asset, dimensions, parity configuration, and initial controlled time, then sought to frame 371. At controlled time 2.4 seconds, both reached frame 419 and emitted one identical full callback vector containing `lingo:introTitleReady()` with all payload fields equal. Their complete RGBA byte vectors were equal, each containing 1,092,000 bytes for 650x420, with SHA-256 `debf718b22be325d22487445c5a41665a9495c634be84ca9ac260da5c5b7b4e3`. The callback is drained only after each continuation, so this check does not manufacture a capture boundary by draining pending work.

## Source-backed boundary and blocker

The diagnostic entry point is `Player::checkpoint_census` in `ruffle/core/src/player.rs:480-619`; its data model is `ruffle/core/src/checkpoint_census.rs:1-192`. It enumerates the root names from `GcRootData` and delegates the basic AVM1, AVM2, and Library observations to `ruffle/core/src/avm1/runtime.rs:113-133`, `ruffle/core/src/avm2.rs:198-208`, and `ruffle/core/src/library.rs:459-473`. The test is `vm-rust/src/native_flash.rs:1114-1259`; player setup and the existing title oracle are in `vm-rust/src/native_flash.rs:327-381` and `:1051-1111`.

The representative result is not a full graph qualification. The display binding adapter now supports this narrow slice, but the object/native/strong/weak census still cannot advance without additional bounded read-only surfaces:

* `ruffle/core/src/avm2/stack.rs:21-29` keeps the stack pointer private and exposes no read-only length/status. A helper there is needed to distinguish dormant AVM2 infrastructure from a live operand/continuation state without executing code.
* `ruffle/core/src/display_object.rs:2548-2565` still limits broader object/property traversal to private `object1` and `object1_or_bare`; the new `checkpoint_existing_avm1_object` adapter is intentionally limited to existing display bindings and does not expose general graph traversal.

Until the remaining AVM2 status adapter and broader display/object adapters exist, display/object/property/native identity, alias/cycle edges, strong and weak liveness, and the 10,000-node/50,000-edge/10,000-weak-edge budgets remain `incomplete_coverage`. Unknown or unclassified state must fail closed. This is a blocker for B2 graph qualification, not a claim that exact restore is impossible.

## Exact execution and verification

The representative command was:

```text
PARITY_OPENING_ANIM_SWF=/Users/clliaw/Projects/childhood-redux/.cache/spybot-recovery/embedded-probe/opening_anim.swf CARGO_TARGET_DIR=$PWD/.cache/b2-census-target mise exec -- cargo test --manifest-path vm-rust/Cargo.toml native_flash_checkpoint_census_is_read_only_before_title_continuation --locked -- --ignored --nocapture
```

The final raw stdout/stderr is retained in [`display-slice-test.log`](./display-slice-test.log), SHA-256 `4d0dbb169e6bc792ea83e034814c17564c5955d6b9ec9f17bdb10f0fc59df5f2`. Its reviewable markers include the 11 `display_avm1` records, `display_traversal: nodes=11 edges=10 ... complete=true stop_reason=None`, full equal callback payloads, equal RGBA hashes, and `test result: ok. 1 passed; 0 failed`.

Because the asset is external to the repository, the focused test is explicitly ignored in normal suites and runs only with the documented `--ignored` command; an absent `PARITY_OPENING_ANIM_SWF` therefore does not cause a normal-suite panic.

The first compile attempt exposed the missing pinned `Stack::len` method in `ruffle/core/src/avm2.rs`; it was repaired by retaining an explicit unknown operand-stack length rather than editing the unapproved stack module. The resulting test passed. A semantic follow-up removed the false live-AVM2 inference: unknown AVM2 state now reports `avm2_execution_state_incomplete`, while `live_avm2_execution` is emitted only for an actually observed live state. The final run passed: one test passed, zero failed, with the census and continuation output above.

The build used the lane's `mise.toml`, Rust `1.98.1`, and the `deterministic`, `audio`, and `mp3` features through the existing `vm-rust` test configuration. The asset was read from the accepted lane artifact and hash-gated. The test process was fresh, but no fresh-worker restore was attempted. Director audio restore was not attempted; the asset has no sound tags, and this does not qualify active Director audio for C1.

## Route and estimate

Recommendation: **narrow** for this display-bound slice. Broad graph expansion and `gc-arena` work are deferred; the full census remains incomplete, and no next component is selected in this packet. The slice result does not authorize a restore implementation or imply full graph eligibility.

The active effort for this display slice and its retained rerun stayed within the approved <=2-hour item. Completing the full root/Library/strong/weak census has a preliminary, not measured, 2–4 active-day estimate. A conditional fresh-Player experiment after an eligible census has a separate preliminary, not measured, additional 2–4 active-day estimate. Neither item is implicitly approved. They exclude a durable codec, full Director session state, active Director audio, and general framework work. Full exact C1 remains unestimated.

The report and receipt are design/diagnostic evidence only. At evidence-review time, no parent commit or push, codec, rehydration, or production source migration had been performed; only the lane-local Ruffle child commit had been published as recorded above.
