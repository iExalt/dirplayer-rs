# Spybot native menu tester spike

Decision date: 2026-09-19.

## Outcome and priority

The user authorized implementation of the native DirPlayer reference and its
bounded evidence probe. The chosen target remains exact comparison of the
existing Bevy Spybot menu through START activation, including deterministic
timing, RGBA, and later PCM qualification. A blocked native probe is accepted
as a dependency checkpoint; it is not menu completion.

The boundary is START activation. Verify source behavior and the one mapped
Bevy StartRequested event before implementing the destination screen. The
destination, tower entry, gameplay, campaign coverage, browser retirement,
worker pooling, concurrent sessions, and broad ownership refactor remain
deferred.

## Scope and constraints

- Use the original Spybot DCR and the normal source startup path.
- Preserve the existing native load guard and one-scenario-per-process worker.
- Keep source mappings and assertions in childhood-redux and runtime semantics
  in dirplayer-rs.
- Stop at the first structured unsupported capability. Do not use a direct-title
  or frame-419 request as a fallback, weaken the guard, or silently waive an
  exact-parity difference.
- Accept P0 only when the first structured result and explicit teardown/reap
  evidence are recorded, or when startup itself fails before a worker exists.

## Source contract

The tested DCR is
`spybot-nightfall-incident.dcr`, SHA-256
`ddf24b667a8d014856d9db42e1658cbf9714847f1cadc2c2c14d21f8e5950c77`.
Director metadata is 650x440 at 20 fps. `BehaviorScript 18 - startFrame.ls`
sets the embedded `opening_anim` Flash sprite to frame 371 and enters
`title`. Flash frame 419 calls `introTitleReady`; its callback then settles
the Director state at `start`. Therefore frame 419 is a transient Flash
`title` observation, while the post-callback Director `start` is a separate
settled state. They must not be collapsed into one checkpoint.

The source button is sprite 6 at (322,381), with top-left (259,370).
`B_ Rollover.ls` maps enter to `_up`, leave to the base member, down to
`_down`, and up to the base member plus `pass`. The attached title behavior
calls `snd_start`, loads data, and transitions to `menu`; the related visual
behavior transitions to `edit`. Those source transitions are semantic
evidence, not destination acceptance.

Recovered aliases include `snd_titlescreen -> s.win_flag`,
`music_intro -> m.v1`, `snd_im_button -> s.select`, and
`snd_start -> s.begin`. `snd_rollover` has no recovered named export.
Its silent behavior is a historical Bevy deviation requiring source/native
qualification; it is not an exact-parity waiver.

## Current capability and dependency evidence

The worker advertises state inspection, virtual input, controlled time, and
RGBA capture. Mutation, invocation, reset, and PCM are unsupported. Input
currently qualifies only stage pointer coordinates and left-button-down
dispatch. Native Flash and audio WIP must be proven by source, build, and
runtime evidence; their presence alone is insufficient. The accepted
external-cast route now has source-root alias/hash input, root confinement,
shell qualification, and owner-bound startup orchestration evidence. It does
not qualify the later `snd_netload_N.cct` payloads or level audio. The current
ownership concern is the legacy `CastHandlers::cast_lib` resolver versus an
explicit `RuntimeSession` context; the next proposed repair is an
explicit-context cast handler using the existing owner/capability machinery.

The dirplayer checkout was tested at HEAD
`2116077c1a5c0d1de896c3863ba01feef0a4a88f`, with nested Ruffle adopted at
`2dfcfedb612509e5d31e7b6e710194b8d3e5e208` on branch
`feat/native-parity`. The childhood-redux checkout was tested at HEAD
`0b2f7d71f0e9480e4fe9c4faee9f13532944e670`. Existing dirty native WIP and
the repository `.DS_Store` entries were preserved and were not staged.

## Executed P0 probe

On 2026-09-19, after a focused harness check and rustfmt check, exactly one
normal source start was run:

```text
PARITY_DIRPLAYER_RS_ROOT=/Users/clliaw/Projects/dirplayer-rs \
PARITY_NATIVE_DIRPLAYER_WORKER=/Users/clliaw/Projects/dirplayer-rs/vm-rust/target/debug/native_dirplayer_parity_worker \
mise exec -- cargo run -p parity --example native_dirplayer_spybot_p0 --locked --offline
```

The harness used separate canonical validation for the dirplayer executable and
source root and the childhood-redux resource/movie root. It sent Discover
request id 1 and exactly one Start request id 2 with protocol version 1,
operation `start`, and the exact DCR configuration recorded in the receipt.
No fallback or direct-title request was sent. The typed first result was:

```text
status: blocked
error code: unsupported
native single-dcr worker does not support external casts:
sound_level_1.cst, sound_level_2.cst, sound_level_3.cst,
sound_level_4.cst, sound_level_5.cst
```

The full Windows source paths are retained in the receipt. The worker then
received exactly one Shutdown request id 3 and acknowledged it. ProcessWorker
termination reaped the process, storage was cleaned, the PID was not alive
after drop, and stderr was empty. The typed transport does not expose raw
response envelopes; the receipt records that protocol-version and request-id
validation limitation.

The reproducible receipt is
[native-start-receipt.json](../../childhood-redux/docs/checkpoints/spybot-native-p0-20260919/native-start-receipt.json)
and records both repo roots, HEADs, full porcelain status entries, nested
Ruffle revision, dependency-closure hashes, harness hash, worker hash, DCR
hash, toolchain, exact envelopes, typed results, and teardown evidence.

P0 is accepted as a structured blocker with clean teardown. It does not prove
Flash completion, frame-419 RGBA, settled Director start, input behavior,
audio, or menu completion.

## Executed qualified-cast probe

On 2026-09-19, the latest native worker was built with the offline locked
toolchain before running a separate qualified-cast probe. The probe supplied
the five audited `sound_level_N.cct` aliases with root-confined relative paths
and exact candidate hashes. It sent Discover request 1, one normal source
Start request 2, and attempted Shutdown request 3. The existing P0 harness and
receipt were not rerun or modified.

The qualified receipt is
[native-qualified-casts-receipt.json](../../childhood-redux/docs/checkpoints/spybot-native-qualified-casts-20260919/native-qualified-casts-receipt.json),
with the discovery note at
[next-gate-analysis.md](../../childhood-redux/docs/checkpoints/spybot-native-qualified-casts-20260919/next-gate-analysis.md).
The worker accepted the alias object far enough to enter ordinary DCR cast
loading, but the receipt does not prove all five aliases were applied because
the main DCR load aborted first. The first Start result was a transport error
after a non-unwinding native abort in `wasm_bindgen::JsValue::from` from
`try_parse_vector_shape` at `vm-rust/src/player/cast_member.rs:4451`.
Shutdown returned `worker is terminated`; termination still reaped the worker,
cleaned storage, and confirmed the PID was dead. This is not a structured Flash
Unsupported result.

## Next component contract

The latest elevated Metal-capable run cleared the renderer gate and localized
the current menu blocker to a stack-localized `foreign or stale DatumRef` panic
at `CastHandlers::cast_lib`. Metal construction progressed, but the production
native Flash scaffold remains unqualified: it does not yet demonstrate frame
advancement, the `introTitleReady` callback, or settled Director `start`.

The next proposed implementation item is an explicit-context cast handler
repair: replace the legacy `CastHandlers::cast_lib` resolution with a typed
`RuntimeSession`-bound path, preserving owner/capability isolation and the
existing qualified shell behavior. No Flash, input, PCM, destination, fallback,
guard-waiver, or broad GPU-discovery item is included.

## Later qualification sequence

After the parser gate is resolved and requalified, the anticipated next gate is
native Flash: compare the source reveal at Flash frame 371, the transient
frame-419 Flash `title`, and the settled Director `start` separately. Current
`NativeFlashHost` semantics cover owner-bound offscreen Ruffle load, initial
render/apply, resize, and unload; they do not yet demonstrate controlled frame
advancement or the `introTitleReady` callback bridge, and load ignores the
paused/asserted-frame inputs. This is a separate medium/high contract. Pointer
and audio/PCM qualification remain later gates.

## Parser-repair rerun

The approved repair replaced only the vector-shape success log with
target-safe `debug!` logging and added focused native parser tests. The two
focused tests passed, the locked offline native worker build passed, and the
installed `wasm32-unknown-unknown` library check passed. The historical P0
and qualified-cast harness and receipt hashes remained byte-identical.

A separate rerun harness and receipt were then used with the same five exact
aliases. It sent Discover request 1, one normal source Start request 2, and
Shutdown request 3. The first Start result was now structured:
`code=runtime`, `native Ruffle offscreen renderer failed: Ruffle does not
support OpenGL on macOS/iOS.` Shutdown was acknowledged; termination reported
`reaped`, storage was cleaned, stderr was empty, and the worker PID was dead.
No fallback request was sent. The rerun receipt is retained at
[native-qualified-casts-rerun-receipt.json](../../childhood-redux/docs/checkpoints/spybot-native-qualified-casts-rerun-20260919/native-qualified-casts-rerun-receipt.json).

The latest elevated run cleared the renderer gate and remains blocked by the
stack-localized `foreign or stale DatumRef` panic at `CastHandlers::cast_lib`.
The tracked GPU discovery establishes the Metal-capable execution route; it
does not qualify Flash behavior. The native Flash scaffold and qualified
external-cast route remain separately unqualified for production menu behavior.
