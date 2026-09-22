# Q0 save-state qualification

Status: **source audit complete; blocked before executable restore qualification**.

Audit date: **2026-09-21**.

This report records the bounded Q0 audit for the controlled-boundary runtime
checkpoint contract. The audit found that the hardest dependency is the live
embedded Ruffle graph. No concrete runtime probe was selected or run: the
pinned public Ruffle boundary has no player graph snapshot or restore operation,
and implementing the prerequisite graph codec and resource rehydration boundary
would be a substantial Ruffle change. That work is outside Q0's allowance and
exclusions. This is a current-boundary finding, not a claim that a future codec
is impossible.

## Baseline and setup

The lane was inspected at:

| Item | Identity |
| --- | --- |
| Worktree | `/Users/clliaw/Projects/dirplayer-rs-save-state` |
| Branch | `save-state` |
| DirPlayer commit | `b481f9f18800290193c235b27daf0e3cb7ad9430` |
| Ruffle gitlink | `fd5d8dda1cc7b8cf91de141a48574750fa8f86b2` |
| Ruffle checkout root | `/Users/clliaw/Projects/dirplayer-rs-save-state/ruffle` |
| Ruffle features | `deterministic`, `audio`, `mp3` from `vm-rust/Cargo.toml` |
| Tool identity | `mise.toml` pins Rust `1.98.1`; no Cargo build was run |
| Assets | No Spybot, SWF, Director, audio, GPU, or worker asset was loaded |

The only setup mutation was initializing this worktree's `ruffle` gitlink at
the exact pinned revision. The unrelated sibling checkout at
`../ruffle` (`2dfcfedb612509e5d31e7b6e710194b8d3e5e208`) was not used. The
checkout root was verified with `git -C ruffle rev-parse --show-toplevel`
before accepting its revision; a bare `git -C ruffle rev-parse HEAD` was not
used as identity proof.

This initialization happened before the navigator redirected Q0 away from the
scanner/runtime check; it was not required for and did not become a build or
restore probe.

## Ownership and barrier inventory

`RuntimeSession` is the owner of the session graph, not just a request
dispatcher. It contains the player graph and owner generations, pending casts,
driver and evaluation continuations, pending commands/completions, deferred
requests, cancellation mailboxes, action and JS registries, native Flash weak
bindings, notification mailboxes, nested-player state, playback controls, and
input/event cleanup state ([`session.rs:295`](../vm-rust/src/player/session.rs#L295),
[`session.rs:301`](../vm-rust/src/player/session.rs#L301),
[`session.rs:313`](../vm-rust/src/player/session.rs#L313),
[`session.rs:331`](../vm-rust/src/player/session.rs#L331),
[`session.rs:367`](../vm-rust/src/player/session.rs#L367)). A returned worker
request cannot stand in for this state.

`DirPlayer` owns mutable Director state including the movie/score, globals and
scopes, command channel, bitmaps, timers, input and drag state, pending cue and
notification queues, audio manager, Flash binding state, active handler and
event flags, playback counters, RNG, caches, XML state, and resource
identities ([`mod.rs:2338`](../vm-rust/src/player/mod.rs#L2338),
[`mod.rs:2350`](../vm-rust/src/player/mod.rs#L2350),
[`mod.rs:2378`](../vm-rust/src/player/mod.rs#L2378),
[`mod.rs:2425`](../vm-rust/src/player/mod.rs#L2425),
[`mod.rs:2499`](../vm-rust/src/player/mod.rs#L2499),
[`mod.rs:2631`](../vm-rust/src/player/mod.rs#L2631),
[`mod.rs:2734`](../vm-rust/src/player/mod.rs#L2734)). A globals-only or current
frame snapshot would lose executable graph identity, pending work, or both.

`NativeFramePump` has independent accepted timestamps, Flash and audio cursors,
the rational 48 kHz sample remainder, cumulative PCM, and callback
observations ([`session.rs:391`](../vm-rust/src/player/session.rs#L391),
[`session.rs:397`](../vm-rust/src/player/session.rs#L397)). These values define
the host-side controlled barrier but do not serialize the source graphs.

`NativeFlashHost` owns a map of live `Arc<Mutex<ruffle_core::Player>>`
instances and a logical clock ([`native_flash.rs:64`](../vm-rust/src/native_flash.rs#L64),
[`native_flash.rs:215`](../vm-rust/src/native_flash.rs#L215)). Its
`NativeFlashSnapshot` contains only sprite, generation, dimensions, and current
frame ([`native_flash.rs:193`](../vm-rust/src/native_flash.rs#L193)). Loading
constructs a new Ruffle player from SWF bytes and an optional asserted frame
([`native_flash.rs:641`](../vm-rust/src/native_flash.rs#L641)); this is useful
startup behavior but is not restoration of a live ActionScript/display graph.

The native Director audio path retains opaque decoder and resampler state in a
boxed `rodio::Source`, plus queued/current segments and channel mutexes
([`native_audio.rs:40`](../vm-rust/src/player/native_audio.rs#L40),
[`native_audio.rs:47`](../vm-rust/src/player/native_audio.rs#L47),
[`native_audio.rs:178`](../vm-rust/src/player/native_audio.rs#L178),
[`native_audio.rs:254`](../vm-rust/src/player/native_audio.rs#L254)). A sample
cursor alone is not established as sufficient for exact PCM continuation.

The pinned Ruffle `Player` contains a private `gc_arena::Arena` holding the
stage, AVM1/AVM2 heaps, action queue, timers, loaders, audio and stream
managers, sockets, local connections, dynamic roots, and post-frame callbacks
([`player.rs:138`](../ruffle/core/src/player.rs#L138),
[`player.rs:180`](../ruffle/core/src/player.rs#L180),
[`player.rs:277`](../ruffle/core/src/player.rs#L277)). It also owns opaque
renderer, audio, navigator, storage, log, UI, and video trait objects
([`player.rs:286`](../ruffle/core/src/player.rs#L286),
[`player.rs:307`](../ruffle/core/src/player.rs#L307)). The public methods
available at this boundary are execution/observation operations such as
`tick`, `handle_event`, `run_frame`, `render`, `current_frame`, and `update`
([`player.rs:530`](../ruffle/core/src/player.rs#L530),
[`player.rs:1084`](../ruffle/core/src/player.rs#L1084),
[`player.rs:2073`](../ruffle/core/src/player.rs#L2073),
[`player.rs:2111`](../ruffle/core/src/player.rs#L2111),
[`player.rs:2167`](../ruffle/core/src/player.rs#L2167),
[`player.rs:2448`](../ruffle/core/src/player.rs#L2448)). No player-level
snapshot/restore entrypoint was found in the inspected pinned source.

Ruffle does have scoped value codecs: AVM2 AMF serialization/deserialization
and LSO handling ([`amf.rs:21`](../ruffle/core/src/avm2/amf.rs#L21),
[`amf.rs:205`](../ruffle/core/src/avm2/amf.rs#L205),
[`amf.rs:313`](../ruffle/core/src/avm2/amf.rs#L313),
[`amf.rs:538`](../ruffle/core/src/avm2/amf.rs#L538)), plus AVM1
`SharedObject` value codecs ([`shared_object.rs:82`](../ruffle/core/src/avm1/globals/shared_object.rs#L82),
[`shared_object.rs:108`](../ruffle/core/src/avm1/globals/shared_object.rs#L108),
[`shared_object.rs:168`](../ruffle/core/src/avm1/globals/shared_object.rs#L168),
[`shared_object.rs:377`](../ruffle/core/src/avm1/globals/shared_object.rs#L377)).
Those codecs cover selected ActionScript values/shared objects; they do not
serialize the `Player`, its GC-root set, continuations, display graph,
timers/action queues, callbacks/loaders, or renderer/audio/backend resources.

As lead review evidence, the targeted search
`rg -n "pub(\\([^)]*\\))?\\s+(async\\s+)?fn\\s+[^\\n]*(snapshot|restore|serialize|deserialize|save_state|load_state)|impl\\s+.*(Serialize|Deserialize).*Player|derive\\([^)]*(Serialize|Deserialize)[^)]*\\).*Player" ruffle/core/src ruffle/core/Cargo.toml`
found only the AVM2 AMF, AVM1 `SharedObject`, and unrelated text-snapshot
matches; it found no `Player` graph API. This review search reinforces the
positive ownership markers above without claiming that a future codec is
impossible.

The controlled-boundary proposal remains: freeze source time after an accepted
operation, complete host/GPU work for that instant, identify due work, and
reject unsupported in-flight capabilities. It cannot turn a safe barrier into
a checkpoint until the live Flash graph, pending callbacks/timers, and active
audio state have a direct representation. Draining to idle, seeking to a frame,
or retaining only RGBA/PCM would change the requested instant or continuation
contract.

## Hypothesis and planned check

Hypothesis: direct C1 restore of the real Spybot instance is blocked at the
current public Ruffle boundary because the live graph is private GC state plus
opaque resources, while the host exposes only execution and observational
operations. Meeting the contract requires a new Ruffle-owned graph/state codec,
stable logical IDs and fixups, and resource rehydration for renderer, callbacks,
loaders, timers, and audio. The exact scope remains unestimated until that
boundary is designed.

The audit was supposed to select one narrow executable feasibility check at the
hardest restore dependency. No concrete runtime probe was selected or executed:
source inspection established first that the pinned public Ruffle boundary has
no operation that can capture and reconstruct the live graph in a fresh player.
An honest executable check of that dependency would therefore require first
implementing the graph codec/resource boundary that Q0 explicitly excludes.
Running startup plus frame seek would be replay or toy playhead substitution,
so it would not test the missing dependency. The full real-Spybot fresh-worker
round trip remains C1 acceptance rather than a Q0 probe. No scanner, runtime
probe, toy restore, replay, second subsystem experiment, or production migration
was created.

## Result and route

The Q0 route is **blocked before narrow executable restore-dependency
qualification**, with a narrow next item:

> Define the Ruffle graph-state codec boundary for the qualified Spybot AVM
> version: identify the actual Spybot AVM version, then produce a type/root/
> resource coverage matrix and candidate fresh-Player rehydration API design.
> The deliverable is an explicit unsupported capability list; it includes no
> codec implementation and is bounded to 1–2 active days.

This route does not reject future implementation. It records that C1 cannot be
qualified or honestly estimated from the current public API. The following is a
preliminary agent estimate from inspected source scope, not a measured estimate:
a limited-workload codec/resource prototype is roughly **2–4+ weeks** after the
matrix is complete. That range covers graph ID/fixup design, qualified
Ruffle graph encode/decode work, host callback/timer reconstruction, and
renderer/audio resource rehydration for the actually identified Spybot AVM;
it excludes broad AVM coverage, arbitrary suspended continuations, and the
full exact C1 contract. Full C1 remains unestimated until graph coverage and
resource semantics are designed. Director-only serialization work should
remain deferred until this dependency is resolved.

Fresh-worker status: **not exercised**. No worker was launched, no checkpoint
was written, no restore was attempted, and no assets were used. C1's required
continuation comparison therefore has no pass/fail evidence.

## Verification and limitations

Commands and observed results:

```text
pwd; git branch --show-current; git rev-parse HEAD; git status --short --branch
/Users/clliaw/Projects/dirplayer-rs-save-state
save-state
b481f9f18800290193c235b27daf0e3cb7ad9430
## save-state

git submodule update --init --checkout ruffle
Submodule path 'ruffle': checked out 'fd5d8dda1cc7b8cf91de141a48574750fa8f86b2'

git -C ruffle rev-parse --show-toplevel
/Users/clliaw/Projects/dirplayer-rs-save-state/ruffle

git -C ruffle rev-parse HEAD
fd5d8dda1cc7b8cf91de141a48574750fa8f86b2

rg -n "^\\s*pub fn (tick|handle_event|run_frame|render|current_frame|update)\\b|gc_arena: Rc<RefCell<GcArena>>|renderer: Box<dyn RenderBackend>|audio: Box<dyn AudioBackend>|navigator: Box<dyn NavigatorBackend>|storage: Box<dyn StorageBackend>" ruffle/core/src/player.rs
Execution/observation methods and private GC/backend markers found; no player-level save/restore method found.

rg -n "struct PreparedSegment|source: Option<Box<dyn Source|struct ChannelState|pub\\(crate\\) struct NativeAudioState" vm-rust/src/player/native_audio.rs
Opaque boxed source, queued/current channel state, and native audio owner found.

rg -n "pub(\\([^)]*\\))?\\s+(async\\s+)?fn\\s+[^\\n]*(snapshot|restore|serialize|deserialize|save_state|load_state)|impl\\s+.*(Serialize|Deserialize).*Player|derive\\([^)]*(Serialize|Deserialize)[^)]*\\).*Player" ruffle/core/src ruffle/core/Cargo.toml
Lead review found AVM2 AMF, AVM1 SharedObject, and unrelated text-snapshot matches; no Player graph API.

git diff --check
pass (after this report was written)

git diff --no-index --check /dev/null docs/Q0_SAVE_STATE_QUALIFICATION.md
pass (exit 1 is the expected untracked-file difference; no whitespace errors)
```

Read-only verification recorded the parent commit, exact Ruffle gitlink and
checkout root, Cargo feature declaration, private Ruffle ownership markers,
public execution API, Director/session ownership fields, and native audio
opaque state. The report links resolve to the inspected source symbols. The
only mutable setup was pinned submodule initialization; no build, network asset
fetch, runtime probe, or source implementation was performed. `git diff
--check` and the untracked-file `--no-index --check` both passed.
