# B2 Ruffle live graph census

Audit date: **2026-09-22**

Status: **eligible and reproducibly pinned census for the actual frame-371 opening/title profile; the later R1 bootstrap prerequisite passed, while mutable fresh-Player restore remains unimplemented and unqualified**.

This B2 component completed a bounded, read-only census of the accepted Spybot AVM1 opening/title state. It does not implement a codec, allocate/fixup/rehydration, durable storage, Director audio state, or production migration. Eligibility applies only to the observed state and exact pinned build/asset identity. A nonempty variant that was empty here must still fail closed until its payload is classified.

The later accepted [R1 bootstrap receipt](../../save-state-r1-20260922/graph/R1_BOOTSTRAP_RECEIPT.md) records source-free private Player construction and logical resolution of all 1,854 actual native builtins. R1 does not restore mutable graph state or retroactively expand B2's census-only result.

The parent census milestone is based on `425af023646653069a7c0bcb18b8040445deebb5` on `save-state`. The published Ruffle census commit is `83c6d65f27a861e7adad3e56837a4fa481ae17fe` on `b2-graph-census`, whose pinned upstream base is `fd5d8dda1cc7b8cf91de141a48574750fa8f86b2`. Its manifest and lockfile pin `https://github.com/iExalt/gc-arena.git` at `682dc66ff12738cd8f3c3c8f268f10c203366240`. The parent `vm-rust/Cargo.lock` carries the same exact source.

The trusted public `iExalt/gc-arena` repository is a fork of `kyren/gc-arena`, retains the upstream CC0/MIT license files and ancestry from `75671ae03f53718357b741ed4027560f14e90836`, and publishes only the narrow `DynamicRootSet::occupied_slot_count` helper plus its test on `b2-dynamic-root-census`. The dependency-first sequence was helper commit, Ruffle pin and census commit, then parent gitlink/evidence. Final focused checks used the checked-in locks with `--locked`, no CLI path patch, and no alternate lockfile. This establishes reproducible source/config dependency resolution for the pinned graph; the ignored runtime test still requires the separately hash-gated read-only Spybot asset.

## Actual asset and boundary

The test hash-gates the accepted [`opening_anim` inventory](../../save-state-b1-20260921/assets/SPYBOT_ASSET_INVENTORY.md):

- source DCR SHA-256: `ddf24b667a8d014856d9db42e1658cbf9714847f1cadc2c2c14d21f8e5950c77`
- embedded SWF SHA-256: `d964a7e594109f8923004333e129f9c655fabde0533a4b4bf74dce238bf3215f`
- parser evidence: `FWS5`, AVM1, 419 frames, 20 fps, zero SWF sound tags

After normal player setup and the pre-first-frame baseline, the test explicitly seeks with `apply_seek(..., 371, false)`/`goto_frame(371, false)`; the census then runs while `FramePhase::Idle` and source time is frozen. The host callback FIFO is already empty. Census code enters the arena for inspection but does not call update, `run_frame`, render, tick, GC, source code, getters, callbacks, or queue drains. The explicit seek advances normal source execution to the selected boundary; no work is drained during census to manufacture eligibility. The ignored, opt-in test is at [`native_flash.rs`](../../../../vm-rust/src/native_flash.rs#L1175), and the Ruffle entry point is [`Player::checkpoint_census_with_baseline`](../../../../ruffle/core/src/player.rs#L860).

## Complete actual-profile result

The final census reports `coverage_complete=true` and `unsupported=[]`. The conjunction requires all 23 `GcRootData` roots, all eight `Library` fields, the display tree, AVM1 graph, action queue, weak liveness, and dormant AVM2 footprint to pass their individual classifications and budgets. The root declaration is in [`player.rs`](../../../../ruffle/core/src/player.rs#L525), the conjunction in [`player.rs`](../../../../ruffle/core/src/player.rs#L1603), and the receipt records the machine-readable result.

The aggregate eligibility ledger reports **9,620 nodes, 5,428 edges, and 5,120 weak entries** against shared 10,000/50,000/10,000 limits. Its named contributions are:

| Category | Nodes | Edges | Weak entries |
|---|---:|---:|---:|
| AVM1 graph | 2,023 | 4,199 | 0 |
| display traversal | 11 | 10 | 0 |
| AVM2 infrastructure and Stage owners | 2,591 | 1,218 | 0 |
| Library | 47 | 0 | 0 |
| action queue | 1 | 1 | 0 |
| strongly retained atoms | 4,947 | — | 0 |
| string interner | — | — | 5,120 |
| other weak owners | — | — | 0 |

These totals are one post-traversal eligibility budget: any total above the shared limit appends a structured aggregate blocker and makes `coverage_complete=false`. They are not a global admission guard across disconnected owner walks. Materialization is bounded by the individual owner limits before the aggregate ledger is formed. Focused tests accept the exact aggregate limit, reject the first node/edge/weak entry beyond it, and verify category composition.

| `GcRootData` root | Observed state | Actual-profile classification |
|---|---:|---|
| `library` | present, 8 fields | all fields classified below |
| `stage` | present, 1 | display children, focus, LoaderInfo, AVM2 Stage, and Stage3D inspected |
| `mouse_data` | empty | hover/button targets absent |
| `drag_object` | empty | no active drag |
| `avm1` | present, 2,023 nodes | persistent roots, display aliases, object edges, native identities, and strings traversed |
| `avm2` | present, 522 playerglobals definitions | execution dormant; pinned infrastructure classified separately |
| `action_queue` | present, 1 | ordered action payload classified without draining |
| `interner` | present, 5,120 live weak slots | every live value has an explicit strong/common owner |
| `load_manager` | empty | no active loader |
| `avm1_shared_objects` | empty | no AVM1 shared object |
| `avm2_shared_objects` | empty | no AVM2 shared object |
| `unbound_text_fields` | empty | no unbound text field |
| `timers` | empty | no active timer |
| `current_context_menu` | empty | no open menu |
| `external_interface` | empty | no provider or registered callback |
| `audio_manager` | empty | no active Flash sound instance |
| `stream_manager` | empty | no active stream |
| `sockets` | empty | no socket or pending socket action |
| `net_connections` | empty | no network connection |
| `local_connections` | empty | no local connection or queued message |
| `orphan_manager` | empty | no orphan entry |
| `dynamic_root` | empty | zero occupied dynamic-root slots |
| `post_frame_callbacks` | empty | no opaque callback |

The eight `Library` fields are enumerated in [`library.rs`](../../../../ruffle/core/src/library.rs#L593):

| `Library` field | Observed state | Classification |
|---|---:|---|
| `movie_libraries` | present, 1 | immutable asset definitions/bindings/fonts/JPEG tables and AVM2 domain descriptor |
| `device_fonts` | empty | a future present resource must be classified |
| `global_fonts` | empty | a future present resource must be classified |
| `font_lookup_cache` | empty | reconstructible query cache when owners are qualified |
| `font_sort_cache` | empty | a future present strong font vector must be classified |
| `default_font_names` | empty | reconstructible pinned configuration when present |
| `default_font_cache` | empty | a future present strong font vector must be classified |
| `avm2_class_registry` | empty | a future live weak binding must be checked for strong retention |

Empty occupancy is an observed state, not a general serialization claim. Each diagnostic owner reports a present unsupported variant rather than silently discarding it.

## AVM1 identity, aliases, native payloads, and weak liveness

The AVM1 traversal assigns census-local integer IDs from object addresses, then emits edges by ID. Addresses are not serialized identities. Repeated pointers reuse the same ID, so aliases and cycles are preserved in the census without recursive duplication. Stored property data, getters, setters, interfaces, watcher callbacks/user data, prototype and native-object edges are visited read-only. This AVM1-only subgraph has 2,023 unique nodes and 4,199 edges, below its 10,000-node and 50,000-edge owner limits; these are not the full-census totals. It records zero AVM1 weak-value observations, no stop path, no native identity blocker, and no collision-budget overflow. The graph builder begins in [`player.rs`](../../../../ruffle/core/src/player.rs#L161); the exhaustive native-variant classifier begins in [`script_object.rs`](../../../../ruffle/core/src/avm1/object/script_object.rs#L882).

The boundary has an empty persistent AVM1 stack and four persistent registers, all `Undefined`. These are persistent runtime roots, distinct from an active activation or execution continuation. The action queue contains one ordered `Construct` item at priority 1/FIFO position 0 for display ordinal 5, with no constructor object, event slices, method object, method arguments, or listener notification payload. The queue is inspected in place and remains pending.

Concrete non-function display-native objects are five `MovieClip` objects at display ordinals 1, 2, 3, 5, and 6. Stage and five Graphics have no AVM1 binding. The display walk has 11 nodes and 10 ordered tree edges. Other native variants are accepted only if their explicit variant classifier succeeds; an observed Date/filter/host resource or opaque Action function would stop the census.

The graph observes 1,342 `Native` and 512 `TableNative` function objects. They are ordinary pinned Ruffle builtins, not arbitrary opaque closures:

- Representative `Native`: `root.case_insensitive.broadcaster_function[0]`, no table index, no separate constructor, inbound alias count 8, and no display-derived canonical path. Its declaration comes from [`as_broadcaster.rs`](../../../../ruffle/core/src/avm1/globals/as_broadcaster.rs#L13).
- Representative `TableNative`: `root.case_insensitive.global_scope[0].Accessibility.isActive`, table index 0, no separate constructor, inbound alias count 1, and no display-derived canonical path. The property/index declaration is in [`accessibility.rs`](../../../../ruffle/core/src/avm1/globals/accessibility.rs#L9), and `ASnative` category 1999 dispatches to that table in [`asnative.rs`](../../../../ruffle/core/src/avm1/globals/asnative.rs#L35).

Every accepted native function has exactly one matching case-qualified bootstrap path, function kind, table index, and constructor-presence tuple in a separate pre-first-frame baseline. It also compares the raw executable and optional constructor function targets to that same-process baseline with function-pointer equality, without logging an address or treating it as a canonical ID. The final run found zero target mismatches, zero missing matches, and zero path collisions; a target change fails closed as `avm1_native_function_target_rebound`. Aliases are real—the broadcaster representative has eight inbound references—but the diagnostic found no display-derived canonical path, and graph IDs retain all alias edges. Its alias flag does not independently prove that every property along a global path is immutable; the same-process raw-target comparison described below guards against a rebound executable. The proposed restore contract is to resolve the whole builtin function object from that unique path in a fresh, exact pinned Player before mutable fixups; it must never serialize or reconstruct a raw Rust function pointer. That resolver/registry and alias fixup are design work only and have not been implemented. A future path is rejected if it lacks the case-qualified bootstrap prefix, a unique baseline match, or exact same-process executable and constructor target equality. Raw target observations are never emitted as canonical or serialized IDs.

The interner contains 5,120 live weak entries, zero dropped entries, and zero entries absent from the enumerated strong/common owners. No GC or weak-to-strong promotion occurs during census. An earlier run reported 11 apparently uncovered values: `_currentframe`, `_droptarget`, `_focusrect`, `_framesloaded`, `_highquality`, `_soundbuftime`, `_totalframes`, `_xmouse`, `_xscale`, `_ymouse`, and `_yscale`. Source review showed all 11 are strongly held names in AVM1 `DisplayPropertyMap`: runtime owns the map at [`runtime.rs`](../../../../ruffle/core/src/avm1/runtime.rs#L68), construction interns the static names at [`stage_object.rs`](../../../../ruffle/core/src/avm1/object/stage_object.rs#L255), and the final owner iterator begins at [`runtime.rs`](../../../../ruffle/core/src/avm1/runtime.rs#L578). Adding that bounded strong-owner enumeration reduced uncovered live weak values from 11 to zero. A genuinely live weak-only target would still reject the boundary; only an actually dead/null weak reference may serialize as dead/null.

## AVM2 and Stage bootstrap state

AVM2 execution is dormant: operand stack 0, scope stack 0, and call stack empty. Initialized infrastructure remains distinct from live AVM2 execution. The diagnostic records:

- playerglobals domain: 522 definitions and 848 classes
- stage domain: zero definitions and zero classes
- native tables: 1,090 methods, 83 allocators, 31 handlers, and 9 constructors
- class aliases: zero in each direction, with reciprocal-map consistency

This footprint is classified `ReconstructiblePinnedBuiltin` only under the exact pinned Ruffle build and playerglobals identity. It is not encoded as mutable AVM2 execution. The footprint and strong-atom owners are inspected in [`avm2.rs`](../../../../ruffle/core/src/avm2.rs#L349).

The source-free baseline is captured in the same fresh test process immediately after root-movie installation and before its first `run_frame`. It records every LoaderInfo scalar/resource field compared at frame 371: stream variant, asset identity, root-display presence/identity, `is_stage`, loader presence/identity, init and complete event flags, `expose_content`, error state, content type, and shared/uncaught event-object presence. For the LoaderInfo base and both event-object bases, the passive adapter enumerates every dynamic property key, enumerability flag, and value; every slot value; every bound-method occupancy and identity; and the prototype, class, and vtable identity. The asset identity is `url=file:///dirplayer-native-embedded.swf;data_len=39012;compressed_len=39033;frames=419;is_movie=true`.

Stage bootstrap also creates, without source constructors, one AVM2 `Stage` object with shape `(values=0, slots=10, bound_methods=0, proto=true)` and a display link, plus four `Stage3D` objects with shape `(0,4,0,true)`, `context3d_present=false`, and `visible=true`. The same exact passive `ScriptObjectData` adapter covers the Stage base and all four Stage3D bases. Public summaries are address-free. Process-local pointer tokens are kept only for exact same-process equality and have a redacted `Debug` implementation; they are neither canonical IDs nor serialization data. Each dynamic-property, slot, and bound-method collection checks its 10,000-field owner limit before materialization. Stage3D enumeration takes at most four entries before collection and retains the full scalar count so a fifth owner fails closed.

A focused actual-asset negative check captures the public Stage3D state, changes populated slot 2, and captures it again. The class, full shape, vtable slot count, and property count remain equal while the slot value differs; the resulting census reports `coverage_complete=false` with `stage3d_changed`. This establishes that equal shapes do not waive mutable content equality. The unconditional construction path is [`Stage::post_instantiation`](../../../../ruffle/core/src/display_object/stage.rs#L859), the bounded Stage3D collection is in [`stage.rs`](../../../../ruffle/core/src/display_object/stage.rs#L310), and the passive DTO adapters are in [`script_object.rs`](../../../../ruffle/core/src/avm2/object/script_object.rs#L195), [`stage_object.rs`](../../../../ruffle/core/src/avm2/object/stage_object.rs#L95), and [`stage3d_object.rs`](../../../../ruffle/core/src/avm2/object/stage3d_object.rs#L54).

The comparison remains diagnostic and same-process. It is accepted for this pinned actual profile because source identifies the pre-first-frame bootstrap origin and the adapters account for the mutable fields above. It does not establish fresh-process identity rebinding or make arbitrary equality sufficient for reconstruction.

## Read-only continuation evidence

The census player and a matched no-census control use the same fresh setup, asset, dimensions, parity configuration, controlled time, and seek to frame 371. Both then continue normally to frame 419 at controlled time 2.4 seconds. They emit the same complete callback vector, in order:

```text
{sprite:1, generation:1, cast_lib:1, cast_member:1, url:"lingo:introTitleReady()"}
```

Both complete RGBA buffers are byte-for-byte equal: 650×420×4 = 1,092,000 bytes, SHA-256 `debf718b22be325d22487445c5a41665a9495c634be84ca9ac260da5c5b7b4e3`. Callbacks are drained only after each continuation. This supports the claim that the census did not change this controlled continuation; it is not restore evidence and does not prove reconstructibility by itself.

The final trusted-dependency stdout/stderr is [`trusted-census-test.log`](./trusted-census-test.log), 529,806 bytes, SHA-256 `d26540dfbe0caab2403c33698771a710c4c0045b4e7ab7ecc498de83b8217665`. The earlier [`complete-census-test.log`](./complete-census-test.log) is the accepted pre-publication run using a CLI path patch, while [`display-slice-test.log`](./display-slice-test.log) and [`baseline-census-test.log`](./baseline-census-test.log) remain historical partial evidence, including the pre-fix 11-value weak-owner gap. Preserving these separately keeps the diagnostic history without treating the path-patched command as the final recipe.

## Commands and checks

The helper itself passed from `gc-arena-b2-census` before publication:

```text
CARGO_TARGET_DIR=$PWD/.target mise exec -- cargo test --locked --offline test_dynamic_root_occupied_slot_count -- --nocapture
```

Result: one passed, zero failed. The trusted branch is `iExalt/gc-arena:b2-dynamic-root-census` at `682dc66ff12738cd8f3c3c8f268f10c203366240`.

After `mise exec -- cargo fetch --locked` resolved that published commit, the focused Ruffle tests ran from `ruffle/` with the checked-in lock:

```text
CARGO_TARGET_DIR=/Users/clliaw/Projects/dirplayer-rs-save-state/.cache/b2-census-target mise exec -- cargo test -p ruffle_core --locked --offline checkpoint_census -- --nocapture
```

Result: four passed, zero failed. These cover aggregate exact-limit/first-over-limit behavior, category composition, and redacted exact-observation debug evidence.

The final real-asset command ran from the parent worktree with its checked-in `vm-rust/Cargo.lock`:

```text
PARITY_OPENING_ANIM_SWF=/Users/clliaw/Projects/childhood-redux/.cache/spybot-recovery/embedded-probe/opening_anim.swf CARGO_TARGET_DIR=/Users/clliaw/Projects/dirplayer-rs-save-state/.cache/b2-census-target mise exec -- cargo test --manifest-path vm-rust/Cargo.toml --locked --offline native_flash_checkpoint_census_is_read_only_before_title_continuation -- --ignored --nocapture
```

Result: the named test passed; the other test binaries ran zero filtered tests. It reproduced the eligible 9,620-node/5,428-edge/5,120-weak-entry census, the equal-shape/different-slot `stage3d_changed` rejection, and the matched callback/exact-RGBA continuation. No CLI dependency patch or alternate lockfile was used. The ignored fixture convention means a normal suite does not require the external asset. The build used the lane `mise.toml`, Rust 1.98.1, offline Cargo after the trusted fetch, and the existing `deterministic`, `audio`, and `mp3` feature configuration.

The historical pre-publication commands and output remain in `complete-census-test.log`; they are evidence of the implementation review, not the final dependency recipe.

## Decision and remaining uncertainty

Recommendation: **go** for a separate bounded fresh-Player allocate/fixup/rehydration experiment, subject to explicit approval. The preliminary estimate from B1 remains 2–4 active days for that experiment; it is an agent estimate, not measured delivery time. Completing this actual-profile census does not authorize restore work and does not estimate full C1.

Unqualified areas remain material:

- no codec, stable durable format, fresh-Player allocator, fixup, publish, rollback, or cleanup exists;
- native builtin path resolution and alias fixup are designed but not implemented;
- present variants of the empty loader/timer/audio/network/host roots remain fail-closed;
- active Director audio is outside this SWF census, and the SWF has no sound tags;
- historical renderer command state and derived GPU caches were not restored or compared across a fresh process;
- no fresh worker or process consumed a checkpoint;
- exact pinned Ruffle/playerglobals, gc-arena helper, and asset identities remain prerequisites;
- the aggregate 10,000/50,000/10,000 limits are post-traversal eligibility totals; owner-specific caps bound materialization, but no single shared admission counter stops all disconnected traversals at the aggregate threshold.

The preserved product goal remains a fresh worker with exact Flash state and active Director audio. Replay is not an accepted substitute.
