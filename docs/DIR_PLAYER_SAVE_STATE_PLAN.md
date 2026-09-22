# DirPlayer durable runtime save states

Status: **Q0 source audit and the accepted B1 source-backed design publication
completed 2026-09-21; the full actual-profile B2 live census and matched-control
continuation passed on 2026-09-22 with reproducibly pinned source/config. The
accepted [R1 bootstrap prerequisite](checkpoints/save-state-r1-20260922/graph/R1_BOOTSTRAP_RECEIPT.md)
now constructs a private source-free Player and resolves all 1,854 actual
frame-371 AVM1 native builtins, while mutable fresh-Player restore remains
unimplemented and unqualified. C1 and C2 remain
unimplemented.** See the [Q0 qualification report](Q0_SAVE_STATE_QUALIFICATION.md),
[B1 graph-boundary design](B1_RUFFLE_BOUNDARY_DESIGN.md), [B1 asset/AVM
inventory](checkpoints/save-state-b1-20260921/assets/SPYBOT_ASSET_INVENTORY.md),
and [B2 live census](checkpoints/save-state-b2-20260921/graph/B2_GRAPH_CENSUS_REPORT.md).
The bounded fresh-Player allocate/fixup/rehydration experiment was subsequently
approved and is in progress one component at a time. Its preliminary, not
measured, 2–4 active-day estimate is not a full C1 estimate. Decisions were made with
agentic-workflow on 2026-09-21.
The [Spybot next-screen spike](../../childhood-redux/docs/SPYBOT_NEXT_SCREEN_SPIKE_PLAN.md) is an independent
parallel workstream; it does not wait for checkpoint support.

## Outcome and decisions

Create durable runtime checkpoints usable as parity fixtures: reach a source
state once, save it, and load it in a fresh worker without replaying the game
from startup. A future final-boss fixture should not require playing the entire
game before every comparison. How that original fixture is reached and verified
remains part of its provenance; this feature does not implement boss gameplay.

The user selected:

- **First milestone:** checkpoint between controlled steps at a defined runtime
  barrier, including a real Spybot instance with embedded Flash and active
  Director audio. Restore must preserve exact continued source state, RGBA, PCM,
  callback delivery, and resource lifetime in a fresh worker.
- **Compatibility:** the same pinned runtime build and identical original assets.
  Reject incompatible files explicitly. Cross-version migration is deferred.
- **Follow-up:** arbitrary-moment snapshots, including an investigation of
  suspended execution and work in flight. Do not claim the first milestone
  satisfies that follow-up by silently moving its requested snapshot time.
- **Priority:** qualify save-state architecture before selecting its first
  implementation item. Run the bounded Spybot next-screen reference spike in
  parallel on its own pinned baseline; neither qualification depends on the other.
- **Initial allowance:** up to three hours for a state/barrier audit and exactly
  one narrow executable feasibility check of the hardest restore dependency;
  return a decision, blockers, and a revised implementation estimate.

These are emulator save states, separate from the game's save-slot format and
cross-launch player preferences. Game save/load, browser removal, desktop input,
world-map rendering, gameplay implementation, and broad ownership refactoring
remain outside this work. In-memory preferences that affect source execution
are nevertheless part of a runtime checkpoint.

## Parallel branches, worktrees, and integration

This supersedes the earlier decision to pause the destination workstream. At
execution setup, create local branches and sibling worktrees as follows. Record
the exact base commits before either experiment; do not switch the primary
checkouts away from `dev` or `main`.

| Repository | Local branch | Sibling worktree | Responsibility | Merge target |
| --- | --- | --- | --- | --- |
| `dirplayer-rs` | `save-state` | `/Users/clliaw/Projects/dirplayer-rs-save-state` | Q0 and subsequently selected checkpoint implementation | `dev` |
| `dirplayer-rs` | `next-screen` | `/Users/clliaw/Projects/dirplayer-rs-next-screen` | Pinned Spybot reference worker, spike instrumentation, any separately justified reference fixes | `dev` |
| `childhood-redux` | `next-screen` | `/Users/clliaw/Projects/childhood-redux-next-screen` | Spybot reference harness/evidence and subsequently selected port implementation | `main` |

Create these at execution kickoff, not as part of this documentation-only planning
change. If a branch or path already exists, inspect it and reuse only if it belongs
to this work; never reset or overwrite unrelated work. Save-state work stays in
DirPlayer unless Q0 identifies a concrete harness dependency. If childhood-redux
changes become necessary for checkpoint testing, give them a separate local
`save-state` branch and `childhood-redux-save-state` sibling worktree from a recorded
`main` commit, with the same isolation and merge rules.

Each lane owns its source edits, Cargo target/build directories, worker binary,
mutable caches, session storage, ports if needed, and capture/output paths. Pin
submodules (including embedded Ruffle) per worktree and resolve any sibling/path
dependencies explicitly; never accidentally build against the other lane or the
primary checkout. Original source assets may be shared read-only with recorded
hashes. Existing untracked recovery assets are not automatically present in new
worktrees: provision verified read-only inputs explicitly. Do not share mutable
preferences or overwrite accepted receipts. Record worker paths, revisions,
features, asset identities, commands and capture times in each receipt.

The next-screen spike reaches its state through normal startup and START input.
It must not depend on save/load operations, a checkpoint format, or unmerged
`save-state` changes. Save-state qualification uses its existing opening/title
workload and must not depend on the new destination port. Share findings through
evidence and small reviewed dependency commits only when demonstrably necessary;
do not merge unfinished branches merely to keep them synchronized. Coordinate
GPU/expensive build use if concurrent runs contend; independent workstreams do
not require concurrent GPU measurements.

Commit and push accomplished milestones on their corresponding topic branches.
Merge each completed, reviewed workstream back to its target after its acceptance
gates pass; a useful spike report may merge without claiming product completion.
The first ready lane may merge first. Before the second integration, reconcile
against the updated target and rerun affected gates on the combined tree: existing
exact title RGBA/PCM/source behavior and isolation, plus next-screen or checkpoint
continuation checks wherever implemented. Preserve branch-specific pre-integration
evidence; a changed runtime build requires new checkpoint fixtures or explicit
incompatibility rejection, never silently relabeled old fixtures. Commit/push the
validated integration. Remove worktrees only after their work is merged and any
untracked evidence is safely retained. No merge is authorized by a merely green
build or a plan document.

## Current evidence and decision-changing uncertainty

The production native worker already has Bevy-owned, ordered host operations,
caller-controlled time, original-source execution, and exact menu RGBA/PCM
evidence. See the [compatibility audit](DIR_PLAYER_CHILDHOOD_REDUX_COMPATIBILITY_AUDIT.md).
Its ownership experiment proves bounded interleaved isolation, not serialization.

Read-only source inspection found:

- [`RuntimeSession`](../vm-rust/src/player/session.rs) owns symbols, players,
  pending commands/casts/completions, continuations, action routes, cancellation
  mailboxes, and weak service bindings. A returned worker request alone is not
  proof that all of these are checkpointable.
- [`DirPlayer`](../vm-rust/src/player/mod.rs) contains mutable VM objects,
  globals/scopes, score/sprites, bitmap state, timers, preferences, and RNG state.
  Saving just globals or the current frame loses executable state and identity.
- `NativeFramePump` retains independent frame/Flash/audio clocks, deadlines,
  rational sample remainder, and cumulative captured PCM.
- [`native_audio.rs`](../vm-rust/src/player/native_audio.rs) retains queued
  segments and opaque Rodio source/decoder state. A playback time or seek offset
  alone is not established as sufficient for exact decoder/resampler recovery.
- [`NativeFlashHost`](../vm-rust/src/native_flash.rs) owns live Ruffle instances.
  Its existing `NativeFlashSnapshot` records sprite/generation/size/playhead only;
  it is an observation DTO, not a restore format.
- The embedded [`Ruffle Player`](../ruffle/core/src/player.rs) contains a GC arena,
  backend trait objects, and callbacks. No general save/restore entrypoint was
  established by the inspected player interface; wider feasibility must be audited.
- The [worker protocol](../vm-rust/src/native_parity_worker.rs) has no save/load
  operation. Native preferences are in-memory, and the parity transport's
  temporary session storage is removed on teardown.

The primary uncertainty is whether the source VM graphs, especially live Ruffle
state, admit a maintainable explicit serialization boundary. Scheduling barriers
alone do not solve that problem. The second is exact audio continuation without
retaining opaque decoder objects or replaying all prior game execution.

Recommended route: explicit versioned logical-state serialization, preserving
graph identity and rebuilding host resources. The strongest alternative is a
deterministic input/event replay fixture. Replay could reduce implementation cost,
but pays prior simulation cost on load and does not meet the selected direct-state
goal. Do not substitute it silently. OS process images/forks do not establish
portable fresh-worker restoration of GPU/device resources either.

## State inventory and checkpoint barrier

Every runtime field must be classified as canonical saved state, immutable
asset reference, reconstructible cache/resource, or unsupported capability.
Absence from the inventory is not permission to discard it.

| Area | Required preservation or reconstruction contract |
| --- | --- |
| Lingo VM and object graph | Globals, lists/property lists, script instances, aliasing/cycles, mutable casts, allocator/symbol identities, score state and any live continuation. Use stable serialized IDs and reference fixups, not addresses. Recompiled code caches may be rebuilt only without executing game logic. |
| Time, RNG and input | Exact integer/rational clocks, frame deadlines, timer order, RNG internal state, pointer/buttons, pressed-sprite capture, hover/focus state and queued input. A seed alone does not capture an advanced RNG. |
| Pending work | Deferred source commands, timers, callback order, completion state, and externally visible notifications. Preserve future work; process due work only through the defined barrier. Explicitly reject unsupported futures, network operations, Xtras, or external effects. |
| Embedded Flash | ActionScript heap/display graph, timelines, nested clips, variables, RNG/time, timers/action queues, loader state, callbacks and host bridge identities. Restoring the playhead and last pixels is insufficient. Qualify the actual Spybot SWF; do not imply all AVM versions/features are supported. |
| Graphics | Mutable bitmaps, palettes, dynamic surfaces, trails, transition buffers, viewport and source render state. GPU handles/readbacks are settled and rebuilt from canonical data. Distinguish historical pixel state from caches; do not assume every surface can be regenerated from the current score. |
| Director audio | Channel admission/mixing order, finite loops/queues, volume/pan/rate, logical sample cursor, rational remainder, decoder/resampler/filter history and queued unconsumed output. Active audio is required, not an unsupported-state escape hatch. |
| Embedded Flash audio | Current host uses Ruffle's null audio backend. Label that existing capability limit; do not claim SWF audio checkpoint support from Director PCM. A future real Flash audio backend must join the checkpoint contract before use. |
| Preferences/external resources | Capture in-memory source-visible preferences and stable original asset identities. Keep checkpoint files outside disposable worker storage. Do not reconnect network requests or replay external side effects automatically on load. |
| Ownership and bindings | Preserve logical relationships while allocating fresh process-local owners/generations and recreating weak bindings. Old handles/callbacks must remain invalid; never revive a retired owner or retain process pointers. |

The proposed v1 barrier runs after an accepted controlled operation with source
time frozen. Complete host/GPU work needed to define that instant, identify all
source work due there, and ensure no unsupported executing handler or opaque
continuation remains. Animations, audio, future timers, and future source actions
may remain active and must be preserved; waiting for the game to become idle is
not an acceptable substitute.

If a requested boundary is not representable, return a structured unsupported-
checkpoint reason with the offending component. Do not silently drain into a
later simulated instant, drop callbacks, stop audio, or reset input. The exact
barrier semantics are an output of qualification, not already implemented.

## File and restore contract

Proposed manifest fields: format/schema version, runtime/build identity, embedded
Ruffle revision and relevant features, source DCR/cast/SWF hashes and logical
paths, relevant rendering/audio configuration, checkpoint time, supported feature
set, and integrity/size metadata. Matching build/assets is necessary; exact output
may also depend on the qualified platform/backend, which must be recorded and
checked rather than promising cross-hardware bit equality.

Write checkpoint data atomically. Validate versions, asset identities, bounds,
object references and capabilities before publishing any restored player. Build
a candidate session privately, allocate/fix up its graphs, rebuild host resources,
then expose it only after successful validation. Failure must clean up partial
resources without replacing a working session or leaving a half-written file.
Fresh-worker restore is the first required path; replacing a live worker's state
is a separate API decision.

Restore must not invoke normal movie startup scripts to recreate the saved state.
The snapshot is authoritative for mutable state. Re-uploading immutable textures
or decoding an original audio clip to reconstruct exact decoder state may be
allowed, with measured cost and proven suffix equivalence. Replaying game inputs,
frame scripts, or all source execution since startup is not direct restore.

Avoid requiring hours of historical output in a late-game fixture. Audit which
audio/video history affects future execution and preserve that state. Prefer an
explicit capture-window API carrying absolute source time/sample positions for
continuation comparisons; historical test output can be a separate artifact.
Do not silently change the existing cumulative PCM protocol. Preserve unconsumed
samples and any source-observable sample history; an accumulator may be omitted
only after its semantics and the versioned capture contract are established.

## Q0: three-hour architecture qualification

The [Q0 source audit](Q0_SAVE_STATE_QUALIFICATION.md) completed on 2026-09-21.
It found that the pinned public Ruffle boundary has no live player-graph capture
or reconstruction operation. No concrete runtime probe was selected or executed
because creating that prerequisite would exceed Q0; the allowed source-established
blocker path below was taken instead. This result does not qualify fresh-worker
restore or implement checkpoint support.

The accepted B1 publication follows this historical Q0 result: its
[asset/AVM inventory](checkpoints/save-state-b1-20260921/assets/SPYBOT_ASSET_INVENTORY.md)
and [Ruffle graph-boundary design](B1_RUFFLE_BOUNDARY_DESIGN.md) resolve the
source-backed design questions for the actual AVM1 title workload. B1 ran no
graph-slice experiment, did not retroactively execute Q0's missing executable
check, and does not qualify fresh-process restore or C1.

The accepted [B2 census report](checkpoints/save-state-b2-20260921/graph/B2_GRAPH_CENSUS_REPORT.md)
dated 2026-09-22 records complete coverage for the trusted actual frame-371
profile: all 23 roots, all eight Library fields, 9,620 aggregate nodes, 5,428
aggregate edges, and 5,120 weak entries. It also records the equal-shape mutable
Stage3D rejection and a matched no-census continuation with identical callback
vector and exact RGBA bytes. The aggregate limits are post-traversal eligibility
totals with bounded owner walks; they are not an early global traversal guard or
an untrusted-input claim. The exact gc-arena and Ruffle commits and both Cargo
locks are reproducibly pinned. Fresh-Player restore remains unimplemented, so
this evidence does not qualify C1. The conditional fresh-Player experiment has
a preliminary, not measured, 2–4 active-day estimate and is not implicitly
approved at the time of B2 publication. The user subsequently approved the
bounded experiment; its accepted R1 bootstrap prerequisite is recorded in the
[R1 receipt](checkpoints/save-state-r1-20260922/graph/R1_BOOTSTRAP_RECEIPT.md).
R1 does not restore mutable graph state or qualify continuation, a fresh worker,
Director audio, or C1.

Q0 was a bounded block, not a promise to build full save states in three hours.
It used one component-level discovery item under subagent-pair-program.

| Work | Allowance | Required output |
| --- | --- | --- |
| State ownership and barrier audit | 60 min | Concrete inventory, existing serialization support, hard blockers and the hardest uncertainty |
| Select/setup/build one feasibility check | 30 min | A written hypothesis, exact state to preserve, affected experimental paths and stop condition |
| Execute and interpret that one check | 60 min | A discriminating result, including what was bypassed and whether fresh-process reconstruction actually occurred |
| Route decision and handoff | 30 min | Go/narrow/blocked recommendation, component-sized next item, revised implementation range and remaining risks |

Prefer testing the live embedded-Flash graph boundary first. If existing code
shows that arbitrary graph serialization requires an unbounded Ruffle subsystem
rewrite, record that blocker rather than spending the remaining time creating a
toy playhead restore. If another dependency is demonstrably the harder blocker,
select it and explain why before the one executable check. A partial graph test
must explicitly identify excluded callbacks, loaders, display objects and audio;
it cannot qualify the real-Spybot milestone.

No production migration, broad ownership repair, new general serialization
framework, or second experimental subsystem belongs in Q0. Setup repairs stay
inside the total three-hour cap. At the cap, stop with evidence and a revised
route; an identified missing capability is a valid qualification result, not
completion of checkpoint support.

## C1: first useful durable checkpoint milestone

After Q0, break this into component-sized implementation items based on the
discovered dependencies. Initial coarse sequence:

1. Define the executable barrier, capability rejection, manifest and graph-ID
   rules. Establish observation of all relevant pending work without altering it.
2. Implement the required Director and embedded-Flash state codecs plus owner
   fixups and resource rehydration. Do not leave Flash behind as an optional stub.
3. Qualify graphics and active audio continuation at exact time/sample positions.
4. Add versioned worker save/load operations, durable fixture storage, and
   comparison-harness integration. Existing worker isolation remains the default.
5. Accept the real Spybot fresh-process round trip and failure/cleanup behavior.

The representative checkpoint should occur during the original opening, near
the title callback, with live Flash and verified active Director audio. Choose
an off-frame/off-sample-grid controlled boundary where feasible so fractional
clock/remainder bugs cannot hide behind aligned steps. Confirm the real workload
before fixing exact timestamps. Include a later held-pointer checkpoint once
the first media continuation works.

Acceptance must compare:

- **Uninterrupted control:** reach the checkpoint and continue through the same
  future inputs, callback boundary, title/menu transition and capture windows.
- **Restored branch:** save, terminate/reap the original worker, launch a fresh
  worker, restore, and perform the identical continuation without startup replay.
- **Repeated branches:** load the same immutable checkpoint more than once and
  prove deterministic continuation and isolation. Compare differing input
  branches against their corresponding uninterrupted controls as well.

Require exact semantic state with logical-ID normalization only where process
ownership intentionally differs; graph aliases/cycles must survive. Require exact
RGBA immediately after restore and at subsequent checkpoints, exact PCM sample
bytes/counts/absolute positions, timer/RNG continuity, callback multiplicity and
ordering, no repeated side effects, correct held-input behavior, and clean
retirement with stale-handle rejection. Saving itself must not change the
uninterrupted continuation.

Test incompatible build/assets, truncated/corrupt files, unsupported in-flight
work, and reconstruction failure. None may publish partial state or damage the
source checkpoint. Measure file size and save/load/setup cost separately from
continued simulation; restore should scale with retained state/resource rebuilding,
not elapsed gameplay. Set numeric performance targets after Q0 measurements.

Existing exact title tests remain regression gates. A matching first frame,
same-process clone, paused renderer, silent audio restart, or manually reconstructed
Spybot globals cannot close C1. Final-boss fixtures remain unqualified until their
actual capabilities and subsequent behavior pass this same contract.

## C2: arbitrary-moment snapshot follow-up

Treat this as a distinct architectural milestone after C1. First distinguish a
snapshot request queued until the next safe barrier from a snapshot that actually
preserves the requested intermediate instant. The former may be useful but does
not satisfy the user's arbitrary-moment goal by itself.

Audit suspended Lingo/ActionScript continuations, stacks/locals, reentrant event
dispatch, host callbacks, pending I/O, and graphics/audio submissions. Determine
which can be represented as explicit continuations/commands and which native
futures or external resources require redesign. Do not serialize Rust stacks,
GPU pointers, OS handles, or closures as raw memory. Preserve event-delivery and
external-effect semantics across the capture boundary.

C2 requires new representative mid-handler, callback, loading and media tests;
C1's clean-boundary evidence does not establish it. Budget, supported capabilities
and implementation sequence remain unestimated until Q0/C1 expose the real cost.
Cross-runtime-upgrade fixture migration remains a separate deferred requirement.

## Decision checklist and handoff

- [x] Select direct runtime checkpoints rather than game saves or full-game replay.
- [x] Select controlled-boundary C1, real Spybot/Flash/active audio, exact continuation,
  fresh workers, and pinned-build/assets compatibility.
- [x] Reserve arbitrary-moment C2 and reject silent safe-point substitution.
- [x] Set the three-hour Q0 cap and isolate parallel `save-state` / `next-screen` worktrees.
- [x] Complete the Q0 source audit and record the blocked/narrow route; no runtime
  qualification was run.
- [x] Publish the accepted B1 source-backed asset/AVM inventory and Ruffle
  graph-boundary design on 2026-09-21; this is docs-only evidence and does not
  implement or qualify C1.
- [x] Accept the historical partial B2 display-bound census and matched-control
  continuation evidence on 2026-09-22.
- [x] Complete and publish the full trusted actual-profile B2 census covering all
  23 roots, eight Library fields, named strong/weak categories, mutable Stage DTO
  rejection, and locked dependency reproduction; this remains diagnostic evidence
  and does not qualify C1.
- [x] Begin the approved bounded fresh-Player experiment and accept R1: an
  isolated source-free Player plus logical resolution of all 1,854 actual
  frame-371 AVM1 native builtins, with no mutable graph restoration.
- [ ] Complete the approved R2 mutable AVM1 plain/script-object capture,
  allocation, and fixup component; display/timeline/resources and continuation
  remain subsequent components. The experiment retains a preliminary, not
  measured, 2–4 active-day estimate and is not a full C1 estimate.
- [ ] Implement and accept C1; commit/push accepted milestones during execution.
- [ ] Merge accepted work back to `dev` and verify interaction with the parallel next-screen lane.
- [ ] Reassess C2 independently of the destination port.

Reconsider when the barrier cannot preserve the selected instant, embedded Flash
needs a substantially larger VM rewrite, exact audio recovery is unavailable,
external side effects cannot be isolated, restore requires replaying the whole
game, or estimates materially grow. Return those tradeoffs to the user; never
weaken continuation equality or omit live subsystems to declare completion.
