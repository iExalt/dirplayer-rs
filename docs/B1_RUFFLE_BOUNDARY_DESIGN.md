# B1 Ruffle graph boundary design

Status: **B1 complete as a source-backed design investigation; the full
actual-profile B2 census and matched-control continuation passed on 2026-09-22
with reproducibly pinned source/config, while fresh-Player restore remains
unimplemented and unqualified**.

This document defines the smallest source-backed boundary that could support a
controlled C1 checkpoint. It does not implement a codec, change Ruffle, or
claim that the asset receipt enumerates the live Ruffle heap/type graph. Entries
distinguish accepted asset facts, the observed runtime endpoint, static source
possibilities, and facts that remain pending the first graph-slice experiment.

## B2 follow-up — 2026-09-22

The accepted [B2 graph census report](checkpoints/save-state-b2-20260921/graph/B2_GRAPH_CENSUS_REPORT.md)
records complete coverage for the trusted actual frame-371 profile: all 23
`GcRootData` roots, all eight `Library` fields, 9,620 aggregate nodes, 5,428
aggregate edges, and 5,120 weak entries. The AVM1 subgraph contributes 2,023
nodes and 4,199 edges; its zero weak observations are not the full weak total.
The matched no-census continuation produced identical callback vectors and
exact 650x420 RGBA bytes. An equal-shape Stage3D slot mutation fails closed as
`stage3d_changed`.

The aggregate 10,000/50,000/10,000 limits are post-traversal eligibility totals
across named owner categories. Individual owner walks bound materialization;
this experiment does not claim a single early global traversal guard or safety
for untrusted inputs. The exact trusted gc-arena and Ruffle commits and both
Cargo locks are published and pass the focused `--locked` checks without a CLI
path patch or alternate lockfile. The opt-in runtime proof still requires the
separately hash-gated read-only asset.

This is census and continuation evidence only. No codec, allocator, fixup,
rehydration, fresh-worker consumption, Director audio restore, or C1
qualification follows from it. A conditional fresh-Player experiment retains a
preliminary, not measured, 2–4 active-day estimate and is not implicitly
approved. The historical B1 statement that no experiment ran in B1 remains
unchanged; this dated follow-up records the later B2 result.

## Baseline and scope

| Item | Identity |
| --- | --- |
| Worktree | `/Users/clliaw/Projects/dirplayer-rs-save-state` |
| Branch and baseline | `save-state`, `68af1562015ac3a565ccecb54d66194be6e29f60` |
| Pinned Ruffle | `fd5d8dda1cc7b8cf91de141a48574750fa8f86b2` |
| Cargo feature identity | `deterministic`, `audio`, `mp3` (`vm-rust/Cargo.toml:81-82`) |
| Tool identity | `mise.toml`, Rust `1.98.1`; no build or runtime probe in B1 |
| Asset identity | Accepted [`SPYBOT_ASSET_INVENTORY.md`](checkpoints/save-state-b1-20260921/assets/SPYBOT_ASSET_INVENTORY.md): DCR SHA-256 `ddf24b667a8d014856d9db42e1658cbf9714847f1cadc2c2c14d21f8e5950c77`; `opening_anim` CASt 210280 -> XMED 220486 -> 12-byte wrapper -> 39,033-byte SWF SHA-256 `d964a7e594109f8923004333e129f9c655fabde0533a4b4bf74dce238bf3215f`; FWS5, 419 frames at 20 fps; AVM1 |

The accepted asset evidence identifies AVM1 as the qualified interpreter
family for this first experiment. It does not enumerate live Ruffle heap nodes,
GC roots, or runtime object kinds. The Q0 audit established that `NativeFlashHost` retains live
`Arc<Mutex<ruffle_core::Player>>` objects but exposes only observation and
execution operations ([`native_flash.rs:64-73`](../vm-rust/src/native_flash.rs#L64),
[`native_flash.rs:193-255`](../vm-rust/src/native_flash.rs#L193)). The pinned
Ruffle `Player` owns a private GC arena and opaque backend trait objects
([`player.rs:286-324`](../ruffle/core/src/player.rs#L286)); the public AMF/LSO
codecs cover selected values/shared objects, not a `Player` graph. B1 therefore
specifies the required internal boundary without implementing it.

Existing serialization support is scoped: AVM2 AMF encodes selected
ActionScript values ([`amf.rs:21`](../ruffle/core/src/avm2/amf.rs#L21),
[`amf.rs:205`](../ruffle/core/src/avm2/amf.rs#L205),
[`amf.rs:313`](../ruffle/core/src/avm2/amf.rs#L313),
[`amf.rs:538`](../ruffle/core/src/avm2/amf.rs#L538)), and AVM1 SharedObject
serializes selected shared-object values ([`shared_object.rs:82`](../ruffle/core/src/avm1/globals/shared_object.rs#L82),
[`shared_object.rs:108`](../ruffle/core/src/avm1/globals/shared_object.rs#L108),
[`shared_object.rs:168`](../ruffle/core/src/avm1/globals/shared_object.rs#L168),
[`shared_object.rs:377`](../ruffle/core/src/avm1/globals/shared_object.rs#L377)).
Those codecs do not cover `Player`, its GC-root set, continuations, display
graph, timers/action queues, callbacks/loaders, or backend resources.

Lead review also recorded the targeted search command
`rg -n "pub(\\([^)]*\\))?\\s+(async\\s+)?fn\\s+[^\\n]*(snapshot|restore|serialize|deserialize|save_state|load_state)|impl\\s+.*(Serialize|Deserialize).*Player|derive\\([^)]*(Serialize|Deserialize)[^)]*\\).*Player" ruffle/core/src ruffle/core/Cargo.toml`.
Its relevant matches were AVM2 AMF, AVM1 `SharedObject`, and unrelated
text-snapshot matches; it found no `Player` graph API. This is lead review
evidence reinforcing the ownership audit, not an executable restore check and
not a proof of impossibility.

## Canonical roots and state classes

The pinned root set is `GcRootData`:

- `library` and `stage` hold immutable SWF definitions plus the mutable display
  graph ([`player.rs:146-154`](../ruffle/core/src/player.rs#L146)). `Library`
  is mixed: movie definitions are manifest references; dynamic fonts, class
  registrations, and mutable movie-library bindings are canonical; pure lookup
  caches rebuild only when that cannot execute host/source effects or change
  behavior; opaque device-font/backend state is unsupported or manifested
  ([`library.rs:429-452`](../ruffle/core/src/library.rs#L429)).
- `mouse_data`, `drag_object`, `avm1`, `avm2`, `action_queue`, and `interner`
  hold input, interpreter, and pending-work state ([`player.rs:155-167`](../ruffle/core/src/player.rs#L155)).
- `load_manager`, shared-object maps, unbound text fields, timers, context
  menu, external interface, audio/stream managers, sockets, network/local
  connections, orphan manager, dynamic roots, and post-frame callbacks are
  additional roots or reachable work ([`player.rs:169-207`](../ruffle/core/src/player.rs#L169)).

The complete `GcRootData` crosswalk is explicit here; no root field is implied
away by the AVM1 asset classification:

| Root field | Ownership and capture | Restore or unsupported rule |
| --- | --- | --- |
| `library` | Mixed root: immutable movie definitions are manifest references; dynamic font/class registrations and mutable movie-library bindings are canonical when present; lookup caches are derived. | Reuse matching pinned definitions, rebuild pure caches only without effects, and fail closed on unclassified mutable subfields or opaque device-font/backend state. |
| `stage` | Canonical display graph root; capture stage properties and strong/weak edges. | Allocate stage/display nodes first, then fix links and weak handles. |
| `mouse_data` | Canonical input/focus state. | Restore logical coordinates/buttons/focus without host events. |
| `drag_object` | Canonical optional drag target and offset. | Restore only when target `GraphId` is strongly retained. |
| `avm1` | Canonical qualified AVM1 state, including persistent values, objects, scopes, stack, and registers. | Restore logical graph; reject live activation/handler continuation. |
| `avm2` | Dormant AVM2 infrastructure for this AVM1 asset; census must record it empty/inactive. | Recreate initialized tables only from build identity; nonempty live AVM2 state is unsupported in B1. |
| `action_queue` | Canonical pending action order and target IDs. | Rehydrate without draining or executing. |
| `interner` | Canonical string identity by encoded value; ordering is schema-defined. | Rebuild intern table from canonical strings; never use process addresses. |
| `load_manager` | Conditional root; active opaque loaders and partial network state are unsupported. | Restore frozen deterministic bytes only; never reconnect or rerun requests. |
| `avm1_shared_objects` | Canonical AVM1 shared-object values keyed by encoded key order. | Restore selected values through the graph codec; unsupported host persistence fails. |
| `avm2_shared_objects` | Dormant for the qualified AVM1 slice; occupancy is censused. | Empty/inactive is accepted; live AVM2 shared-object state is outside B1. |
| `unbound_text_fields` | Canonical list of text fields and unresolved bindings. | Restore target IDs and binding metadata; unresolved opaque binding fails. |
| `timers` | Canonical logical deadlines, IDs, callback identity, and parameters. | Rebuild heap without firing callbacks. |
| `current_context_menu` | Canonical source-visible menu flags/items when present. | Restore logical menu state; host UI handles are rebuilt or omitted. |
| `external_interface` | Unsupported by default because host callbacks/effects are opaque. | Accept only registered logical callback IDs with no in-flight effect. |
| `audio_manager` | Unsupported Flash audio state; no sound tags occur in this asset. | Restore null-audio configuration only; active Flash audio needs a separate codec. |
| `stream_manager` | Unsupported active stream/decode state unless frozen bytes and cursor codec exist. | Reject active opaque streams in B1. |
| `sockets` | Unsupported external I/O and pending futures. | Do not reconnect or replay network effects. |
| `net_connections` | Unsupported external connection state. | Reject active connections; logical local metadata alone is insufficient. |
| `local_connections` | Conditional local message state; callback order must be logical and deterministic. | B1 rejects opaque pending messages/effects. |
| `orphan_manager` | Canonical membership plus weak liveness; weak entries never become roots. | Restore only an actually nil/dead weak reference or a target ID already retained by the strong graph; a live weak-only target is unsupported. |
| `dynamic_root` | Canonical logical host handles and retained object IDs. | Rebind fresh worker handles; unknown external handles fail. |
| `post_frame_callbacks` | Unsupported because each entry contains an opaque `FnOnce` closure. | Require empty at capture; do not invoke or serialize closures. |

The AVM1 object graph beneath `avm1` is also bounded by the pinned variants:
`ObjectData` contains native kind, property map, interfaces, and watchers
([`script_object.rs:93-100`](../ruffle/core/src/avm1/object/script_object.rs#L93));
`NativeObject` covers `None`, `Super`, `Bool`, `Number`, `String`, `Array`,
`Function`, `MovieClip`, `Button`, `EditText`, `Video`, `Date`, all pinned
filter variants, `ColorTransform`, `Transform`, `TextFormat`, `NetStream`,
`BitmapData`, `Xml`, `XmlNode`, `SharedObject`, `XmlSocket`, `FileReference`,
`NetConnection`, `LocalConnection`, `Sound`, `StyleSheet`, and
`TextSnapshot` ([`object.rs:124-177`](../ruffle/core/src/avm1/object.rs#L124)).
These are script objects, display objects, value wrappers, media, XML, shared
objects, and host wrappers. Script object, function, prototype, and scope
identity use stable per-capture IDs and ordered property keys. Bytecode functions retain immutable SWF slice/build identity and
captured scope/prototype edges. Native builtins are immutable identities keyed
to the pinned build and a versioned builtin registry; their Rust function
pointers are never serialized. Host wrappers are reconstructible only when
their logical owner/generation is registered. Any opaque native variant,
unregistered wrapper, watcher closure, or unclassified object fails closed.
Function representation is explicit in `NativeFunction`,
`TableNativeFunction`, `Executable`, and `FunctionObject`
([`function.rs:20-33`](../ruffle/core/src/avm1/function.rs#L20),
[`function.rs:417-465`](../ruffle/core/src/avm1/function.rs#L417)); native and
table-native entries use the build registry, while `Action` entries use the
immutable AVM1 code slice and graph references.

The outer `Player` adds scalar execution state, parity clocks, input, RNG,
frame accumulator, current frame, movie identity, and backend objects
([`player.rs:286-396`](../ruffle/core/src/player.rs#L286)). The classification is:

| State class | B1 rule |
| --- | --- |
| Canonical saved state | Mutable source-visible values and graph edges whose future behavior can differ: display topology/properties, AVM heaps/scopes/stacks, RNG, clocks, timers, action queues, callback order, input capture, owner-visible logical bindings, and active media cursors once their codec is proven. |
| Immutable asset reference | Exact SWF/DCR/cast/SWF bytes, tag/ABC/code slices, static native tables, build/features, and source asset hashes. References are manifest-checked; bytes are not regenerated by startup. |
| Reconstructible resource/cache | GPU handles, derived command buffers, GPU cache entries, viewport objects, backend instances, weak process references, notification senders, wall-clock instants, and derived lookup/culling caches, rebuilt only from canonical state without source execution. Persistent bitmap/surface pixels, dynamic drawing commands/state, masks, trails, filters, and other render history that affects future output remain canonical. |
| Unsupported capability | Active opaque futures, network/socket effects, arbitrary Rust closures, unknown native callbacks, unsupported AVM/domain types, Flash audio state unavailable from the null backend, and any node not in the versioned capability registry. Capture rejects these before publication. |

The human-readable machine-auditable row matrix is at
[`graph-boundary-matrix.json`](checkpoints/save-state-b1-20260921/graph/graph-boundary-matrix.json).

## Reachable graph surfaces

### Display graph and identity

`StageData` owns children, AVM2 stage object, loader info, Stage3D objects,
focus, movie, viewport and stage configuration
([`stage.rs:41-130`](../ruffle/core/src/display_object/stage.rs#L41)).
`DisplayObjectBase` owns parent, placement/depth/name, transforms, masks,
metadata, flags, scroll rectangles, filters, shaders, and bitmap cache state
([`display_object.rs:259-330`](../ruffle/core/src/display_object.rs#L259)).
`MovieClipData` adds AVM1/AVM2 object links, drop/hit targets, queued tags,
attached audio, execution-list links, event handlers, drawing, queued/current
frames, flags, and pending scripts
([`movie_clip.rs:163-229`](../ruffle/core/src/display_object/movie_clip.rs#L163)).

The accepted `opening_anim` parse observes `DefineSprite`, `DefineShape`,
`DefineBitsJPEG2`, `DefineBitsLossless2`, `PlaceObject2`, and `RemoveObject2`
tags. These establish display-asset inputs, while the exact live Ruffle node
kinds, aliases, and cycles remain `first-experiment-pending`; the asset parser
does not enumerate the runtime heap.

The capture walk must start at ordered roots (stage, AVM global roots, dynamic
roots, shared objects, pending-work targets, and host bindings), then visit
strong and weak edges. Child containers are ordered by the serialized display
list/depth order; map-like collections use an explicit key order. Pointer
addresses and Rust `HashMap` iteration are never identity inputs.

### AVM1, AVM2, functions, and prototypes

AVM1 retains player version, constant pool, global environments, display
properties, operand stack, registers, halted state, execution-list head, and
semantic flags ([`avm1/runtime.rs:53-111`](../ruffle/core/src/avm1/runtime.rs#L53)).
AVM2 retains operand/scope/call stacks, domains, global/class state, namespace
tables, native method tables, broadcast weak references, and class aliases
([`avm2.rs:114-180`](../ruffle/core/src/avm2.rs#L114)). AVM1 functions retain
SWF slices, names, parameters, captured scope/constant pool, and base clip
([`avm1/function.rs:49-83`](../ruffle/core/src/avm1/function.rs#L49)); AVM2
bound methods retain method, scope chain, receiver, and superclass
([`avm2/function.rs:15-39`](../ruffle/core/src/avm2/function.rs#L15)).

The pinned AVM1 implementation separates persistent runtime state from a live
activation. `Avm1` owns the shared operand `stack` and four shared `registers`
([`runtime.rs:55-76`](../ruffle/core/src/avm1/runtime.rs#L55)); its `clear`
method resets both after each queued action
([`runtime.rs:328-336`](../ruffle/core/src/avm1/runtime.rs#L328)). An
`Activation` instead owns the current scope, `this`, callee, local-register
slice, target clip, and mutable update context
([`activation.rs:130-185`](../ruffle/core/src/avm1/activation.rs#L130));
`Player::run_actions` creates these frames and clears persistent leftovers only
after each action ([`player.rs:2222-2302`](../ruffle/core/src/player.rs#L2222)).
The eligibility hook must therefore capture persistent `Avm1.stack` and
`Avm1.registers` when their values remain semantically live between actions;
it must not require the persistent register array to be empty. A current
`Activation`, stack-local register slice, or executing handler is C1
unsupported and must be absent at the selected idle boundary. The first slice
does not serialize a suspended activation or handler continuation.

The qualified opening/title asset is AVM1: the SWF5 header and three `DoAction`
tags are present. `DoInitAction` is separately absent and is another AVM1
action tag; the AVM2-related tags `DoABC`, `DoABC2`, and `SymbolClass` are also
absent. AVM2 remains a static-source possibility but is out of this first
experiment and untested. This asset fact narrows the initial graph surface;
it does not establish which AVM1 runtime objects are live at the boundary.

These fields imply stable IDs for objects, scopes, prototypes, functions,
receivers, constant pools, and weak broadcast entries. Persistent AVM1
operand/register values belong to canonical state when their semantics survive
between actions; a live activation frame, executing handler, or AVM2 call/
operand continuation is C2-style state and is fail-closed. B1 does not design
arbitrary continuation serialization. A clean barrier may retain future timers
and queued actions, but it must not capture an executing handler or an opaque
Rust continuation.

### Queues, timers, loaders, and host resources

`ActionQueue` contains three priority buckets of target clips and action types
([`context.rs:477-536`](../ruffle/core/src/context.rs#L477)). `Timers` contains
a binary heap, timer counter, logical time, and callback variants that retain
function/method identity and parameters ([`timer.rs:21-33`](../ruffle/core/src/timer.rs#L21)).
`LoadManager` stores in-progress loaders, target clips, request/status state,
partial movie state, and asynchronous load processes
([`loader.rs:215-299`](../ruffle/core/src/loader.rs#L215)).

Queued actions and timers are canonical only when their callback and target
references resolve through the capability registry. An active loader, socket,
or network connection is unsupported unless its bytes, deterministic request
metadata, pending callback order, and no-external-effect semantics are all
represented. Reconnecting or reissuing a request on restore is forbidden.

`PostFrameCallback` contains a `Box<dyn FnOnce>` and a display object
([`player.rs:209-216`](../ruffle/core/src/player.rs#L209)); arbitrary callback
closures are unsupported for C1. `ExternalInterface` callbacks and navigator
operations likewise require an explicit logical callback ID and host capability;
otherwise capture rejects them. Native Flash callback FIFO entries are
owner-qualified host state ([`native_flash.rs:75-147`](../vm-rust/src/native_flash.rs#L75));
their order may be canonical, while old owner handles are always invalidated.
Capture never calls `take_callbacks` or otherwise drains this FIFO to manufacture
a barrier ([`native_flash.rs:584-597`](../vm-rust/src/native_flash.rs#L584)).
B1 chooses structured unsupported for a nonempty native host FIFO; a later
codec may preserve its ordered entries with fresh owner/generation rebinding.

### Runtime chain, time, RNG, rendering, and audio

`Player` exposes parity time and seed setters but stores additional frame
accumulator, run state, timer deadline, and RNG state
([`player.rs:322-365`](../ruffle/core/src/player.rs#L322)). The checkpoint must
save exact integer/rational logical clocks and RNG state, not wall-clock
`Instant` values or a seed alone. The native Director pump separately retains
Flash/audio cursors and a rational PCM remainder
([`session.rs:397-412`](../vm-rust/src/player/session.rs#L397)).

The hash-gated runtime chain is defined by
[`native_flash.rs:1051-1111`](../vm-rust/src/native_flash.rs#L1051): seek
`opening_anim` to frame 371, advance 2,400,000 microseconds, then require frame
419 and `lingo:introTitleReady()`. The accepted full-DCR receipt confirms the
exact DCR hash, Director frame 6/start, Flash 371->419, one callback at 2.4 s,
and teardown on that DCR. Its lane-independent receipt is
`/Users/clliaw/Projects/dirplayer-rs/vm-rust/.cache/native-bevy-schedule-probe-20260921/receipt.json`
with SHA-256 `da87aada0fdb1c1b6c025a133bcc93af9cfbeb019ad0cb73a1582d1d30fade9e`.
This is an observed runtime endpoint, not original Director evidence, and the
receipt does not enumerate live Ruffle graph types.

Renderer/audio/navigator/storage/UI/video fields are trait objects
([`player.rs:312-318`](../ruffle/core/src/player.rs#L312)). Derived GPU
handles, command buffers, and cache entries are reconstructible resources.
Persistent render history that can affect future output or source observation is
canonical; its concrete fields are enumerated in the matrix. Ruffle's
default `NullAudioBackend` is the current Flash-audio capability boundary
([`backend/audio.rs:199-232`](../ruffle/core/src/backend/audio.rs#L199)); it
does not prove Flash audio continuation. Active Director audio remains an
independent required C1 dependency: its opaque native decoder/resampler state
is outside this B1 graph experiment.

No Flash sound-related tags were present in the accepted `opening_anim` asset,
so this actual graph-slice experiment need not restore Flash audio. That fact
does not qualify the null backend and does not remove active Director audio
from the exact C1 contract.

## Stable IDs, capability detection, and capture eligibility

### Graph ID algorithm

The proposed per-capture algorithm is:

1. Validate the manifest, build identity, feature identity, asset hashes, and
   supported capability set before traversing.
2. Establish an ordered root list: stage; AVM1/AVM2 global roots; shared-object
   roots sorted by encoded key; dynamic roots in their explicit insertion/order
   representation; and pending-work/host roots sorted by queue order and stable
   logical key. The exact root adapters must be implemented inside Ruffle,
   because `GcRootData` and its fields are private to `ruffle_core`.
3. Traverse strong edges in schema-defined field order. Sort keyed collections
   by canonical encoded key plus a deterministic tie-breaker; never use
   `HashMap` iteration or process addresses. Display children use depth/list
   order, and queues use priority/FIFO order.
4. Assign a `GraphId` on first encounter and emit forward references when an
   edge points to a not-yet-emitted node. A pointer encountered again must reuse
   the existing ID; a duplicate node with conflicting kind/schema is a capture
   error.
5. Determine each weak reference's actual liveness without promoting it. Record
   `nil`/dead only when the source weak reference is actually nil/dead. Record a
   `GraphId` only when the live target is already retained by a strong root or
   edge. A live target absent from the strong graph is
   `Unsupported(weak_target_not_strongly_retained)`; it is never reclassified as
   dead and never promoted to a root. GC/process addresses may be ephemeral keys
   in the capture-time identity map for alias detection, but never serialized
   IDs or ordering inputs. Weak-only liveness qualification is a separate future
   item.
6. Consult a versioned node-kind/capability registry before emitting each node.
   Unknown node kinds, unknown fields, unsupported native/closure payloads,
   nonempty execution stacks, active handlers, opaque futures, and unbounded
   external effects fail capture before source execution or checkpoint
   publication.
7. Emit a graph digest over ordered node records and edges. The digest is a
   diagnostic oracle; it is not a replacement for restoring canonical fields.

This two-phase identity process preserves aliases and cycles while making
process ownership irrelevant. It also detects latent unsupported state during
capture rather than discovering it after partial restore.

### Capture eligibility predicate

The current host cannot inspect the private `Player::frame_phase` field or the
private `GcRootData` queues through the public API. `UpdateContext` exposes a
mutable `frame_phase` only inside `Player::mutate_with_update_context`
([`context.rs:210-213`](../ruffle/core/src/context.rs#L210)); the enum defines
`Idle` as the non-frame-processing phase
([`frame_lifecycle.rs:21-65`](../ruffle/core/src/frame_lifecycle.rs#L21)).
`Player::run_frame` also drains actions, updates local connections, executes
post-frame callbacks, and marks rendering needed
([`player.rs:2073-2108`](../ruffle/core/src/player.rs#L2073)). Therefore B1
does not assume that `FramePhase::Idle` is externally testable or sufficient.

A required future internal hook is:

```text
Player::checkpoint_eligibility() -> Result<CheckpointEligibility, Unsupported>
```

implemented inside `ruffle_core`, alongside the private root access. It must
inspect without running source and return a structured result only when all of
these hold:

- the player is at the defined clean frame boundary, with the internal phase
  observed as `Idle` through the private access path;
- no AVM1/AVM2 handler or current activation is executing; persistent AVM1
  operand/register data is captured when it remains semantically live, while
  stack-local activation frames and opaque AVM2 call/operand continuations are
  absent from the C1 profile;
- no frame-script cleanup queue, unsupported post-frame closure, opaque future,
  active network/socket effect, or unsupported loader remains;
- action/timer queues, callbacks, display roots, dynamic roots, and owner
  bindings are enumerable through the graph schema; and
- renderer/resource state is either canonical or rebuildable under the
  manifest's feature set.

The host-side predicate additionally requires the Director operation to have
returned, the native Flash callback FIFO to already be empty at the selected
post-accepted-operation point, owner and generation validation to still pass,
and no pending session command/completion to be in-flight. The frame-419
callback remains in Ruffle's canonical future state until its controlled
operation; capture does not drain it. B1 returns structured unsupported for a
nonempty host FIFO. Nonempty future timer/action queues and active Director
audio are allowed only if their canonical codecs are independently supported.
Any failed clause returns a named unsupported reason; it does not drain work or
move the requested instant.

## Fresh-player restore contract

The required API is an internal checkpoint-mode construction path, separate
from normal `PlayerBuilder::build` plus `set_root_movie`. Current native Flash
construction installs the root movie, runs the first frame, and renders
([`native_flash.rs:350-379`](../vm-rust/src/native_flash.rs#L350)); using it for
restore would execute startup side effects. The future path is:

1. Validate manifest/schema, runtime and Ruffle identity, asset hashes,
   dimensions, bounds, capability registry, and graph digest metadata.
2. Allocate a private empty candidate player with matching immutable SWF/build
   references, parity configuration, backend configuration, and fresh owner.
   Do not install the movie through the normal startup path.
3. Allocate strong graph nodes by `GraphId`, then populate scalar fields and
   immutable references. Resolve forward references in a separate fixup phase so
   cycles and aliases are preserved. Rebuild weak handles afterward only as
   serialized actual nil/dead state or a `GraphId` already retained by the
   strong graph; reject live weak-only targets and never promote a weak target
   during restore.
4. Rebuild render targets, bitmap resources, host callback buffers, storage and
   other process-local resources from canonical data. Rebind callbacks with a
   fresh owner/generation; never revive old process pointers or handles.
5. Rehydrate timers, action queues, input/focus/drag state, logical clocks, RNG,
   and future media state. Reject unsupported in-flight work before any source
   execution.
6. Validate graph invariants, weak-edge targets, queue ordering, callback
   multiplicity/order metadata, asset identity, resource completeness, and
   stale-handle fences.
7. Publish the candidate atomically into the fresh worker only after every
   validation succeeds. Drop all partial candidate resources on failure and
   leave the source checkpoint untouched.

This is a design contract, not an implementation. C1 covers a clean controlled
barrier. Arbitrary suspended continuations, live activation frames, executing
handlers, and opaque Rust futures remain C2 or unsupported until a separate
design qualifies them; persistent AVM1 stack/register values with surviving
semantics are captured by the qualified hook.

## First executable experiment after B1

The first experiment after this completed design should be one in-memory or
lane-local graph-slice round trip for the actual AVM1 opening/title Ruffle
state identified in the accepted asset evidence:

- first run a no-source-execution live census/preflight over all 23
  `GcRootData` fields, including `Library` subfield occupancy, root occupancy, AVM1 `NativeObject` kinds, strong and
  weak edge counts/liveness, persistent AVM1 stack/register values, dormant
  AVM2 occupancy, and pending action/timer/loader/host capabilities;
- preflight passes only when the census is complete, the graph is within the
  declared budget of 10,000 nodes, 50,000 edges, and 10,000 weak edges, zero
  unsupported capabilities are observed, the host FIFO is already empty, no
  `Activation`/handler is live, and every persistent value has a logical rule;
- preflight fails before codec work on any unsupported or opaque kind, budget
  excess, incomplete root/edge census, nonempty host FIFO, live activation or
  handler, or nonempty live AVM2 infrastructure; no callback is drained to make
  the check pass;
- no durable checkpoint format;
- no Director session codec or Director audio restore; the asset has no Flash
  sound tags, so Flash audio is not part of this slice;
- no replay and no playhead/RGBA-only substitute;
- a fresh checkpoint-mode `Player` receives a bounded graph slice captured
  without executing source during capture;
- the slice includes whichever real opening/title roots are present, plus at
  least one alias/cycle, pending-order entry, and callback logical identity if
  the actual state has them; absent features are recorded rather than invented;
- restore performs allocation, fixups, resource rehydration, and validation
  without normal startup scripts; and
- one controlled future Flash operation supplies the continuation oracle.

Pass requires exact normalized graph topology, alias/cycle multiplicity,
pending action/timer order, callback logical identity/order, no startup side
effects, and matching result of the one controlled continuation. Fail requires
any omitted reachable node, changed alias/cycle edge, reordered pending work,
repeated callback, unreported unsupported capability, or inability to publish a
fully validated fresh player. The experiment does not claim C1 because active
Director audio remains an independent required dependency and the current
Ruffle null audio backend leaves Flash audio outside the observed capability.
The preflight must produce a pass/fail receipt by the first active day; two
nonadvancing setup/repair attempts also stop the item as `blocked`.

No experiment is run in B1. It must stop if the actual state contains a
node kind, closure, loader, external effect, or resource that the registry
cannot classify, or if the required internal Ruffle hook would itself become a
general codec/framework effort. The next decision is then `blocked` with the
specific unsupported capability, not a weakened pixel/replay result.

## Sequence, estimate, and decision

B1 remains complete as a source-backed design investigation. The later B2 item
completed the no-source-execution census/preflight for the trusted actual
frame-371 profile. It did not implement the checkpoint-mode
allocate/fixup/rehydrate/publish path that B1 paired with the original broader
2–4 day graph-slice estimate.

The current sequence is:

1. Accepted DCR/SWF identity and AVM1 asset evidence — complete.
2. B1 source-backed graph boundary, capability registry, and barrier/restore
   design — complete.
3. Full actual-profile live census/preflight with bounded owner walks, aggregate
   eligibility, mutable Stage DTO rejection, weak-liveness evidence, and locked
   trusted dependency pins — complete in B2.
4. Fresh checkpoint-mode `Player` allocation, fixup, rehydration, validation,
   publication, cleanup, and one continuation oracle — next only if separately
   approved; preliminary, not measured, **2–4 active days**. It must stop on an
   unclassified root or if the work expands into a general codec/framework.
5. If that oracle passes, a limited-workload Ruffle graph/resource prototype —
   preliminary **2–4+ weeks**, covering codec schema, graph allocation/fixups,
   qualified AVM1 state, callback/timer handling, and resource rebuilding. This
   remains an estimate with the assumption that the experiment finds no new
   opaque roots; it excludes Director audio and full C1.
6. Director audio/session integration and exact C1 qualification remain
   separately sequenced and **unestimated**.

Recommendation: **go** to the separately approved bounded fresh-Player
experiment. The census resolves the actual-profile ownership question without
qualifying reconstruction. Exact pinned build/assets, fail-closed present-root
handling, no startup source execution, fresh logical identities, and the
controlled continuation oracle remain required. No codec or restore work is
authorized by B1 or B2 publication alone.
