# Spybot native menu tester spike

Decision date: 2026-09-19.

## Current checkpoint (2026-09-21)

The published baselines are dirplayer-rs `e82c6992` and childhood-redux
`82c02c5`; the active nested Ruffle used by the current native visual recipe is
`fd5d8dda`. Native source rasterization is canonical for opening frames
371–419, with Bevy scheduling, input, state, and audio remaining independent.
The normal source path reaches Flash 371, advances through 419, dispatches the
owned `introTitleReady` callback, and settles Director at `start`. The native
worker now supports owned state inspection and invocation, deterministic time,
RGBA and cumulative 48 kHz stereo PCM capture, viewport configuration,
source-backed external casts, session-isolated preferences, native Flash seek
and advancement, Director rateShift, and menu pointer move/down/up with capture
and cancellation. Available-DCR rollover silence is qualified by the recovered
source and the browser handler comparison; no replacement cue is invented.

The user-selected visual contract uses pinned native Ruffle source artwork for
frames 371–419 while Bevy keeps independent scheduling, input, state, and
audio. Representative native/Bevy pixels are exact at the accepted checkpoints.
Native combined/split PCM is repeatable, and Bevy now admits title audio at the
source 50 ms boundary, but exact native-versus-Bevy PCM still differs; the
decoder/resampler policy is pending. The complete comparison campaign has not
run, so final fidelity remains open. All later checkpoint sections are
historical evidence unless this section explicitly promotes their result; in
particular, the experimental cast-property fast path was reverted.

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
For the available DCR, recovered-source dispatch and the matched browser
handler comparison qualify no added rollover audio; no substitute cue is used.

## Current capability and dependency evidence

The current worker capabilities are the accepted set summarized above. Typed
invocation runs through the existing owned command queue rather than string
evaluation. Pointer move/down/up preserves capture, release cancellation,
replacement, and owner retirement. PCM capture covers the demonstrated title,
select, and begin paths, including source rateShift; it does not qualify later
`snd_netload_N.cct` level audio. External-cast aliases remain hash-bound,
root-confined, and owner-qualified. The explicit-context cast repair is
accepted; the former stale-DatumRef blocker and cast-property fast path are
historical checkpoints, not current work.

The next boundary is the user decision and implementation needed for exact
native-versus-Bevy PCM decoding/resampling, followed by the full normal-source
and direct diagnostic comparison campaign. Direct-title/frame requests remain
diagnostic-only and cannot replace normal startup evidence.

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

## Historical explicit-context checkpoint

Published baselines for this checkpoint are dirplayer-rs `e82c6992`
(`e82c69920e36c970ba20e41bb0b14de16669e832`) and childhood-redux `82c02c5`
(`82c02c52d57fffd9f4af4402043fc76d8a1a9f01`); older identities above are
historical pre-publication evidence.

On 2026-09-19, the approved narrow repair changed only
`CastHandlers::cast_lib` and `CastHandlers::find_empty` to consume the explicit
`ExecutionContext` passed by `BuiltInHandlerManager`; allocator owner and
generation guards remain in place. The focused player-2 ownership regression,
the manager ownership tests, and the existing `cast_lib` guard tests passed.
The locked offline native worker build also passed.

The one approved elevated normal-source qualification was attempted with
`RUST_BACKTRACE=full` and the distinct proposed receipt path
`docs/checkpoints/spybot-native-explicit-cast-context-20260919/native-explicit-cast-context-receipt.json`.
The receipt recorded Discover request 1 and exactly one normal-source Start
request 2. The first Start result was a transport timeout after 30 seconds;
no fallback request was sent. Teardown attempted Shutdown request 3, received
`worker is terminated`, reaped the worker, cleaned storage, left empty stderr,
and confirmed `worker_alive_after_drop: false`. The receipt is
`docs/checkpoints/spybot-native-explicit-cast-context-20260919/native-explicit-cast-context-receipt.json`.
Historical receipt identities and contents were not rerun or modified.

## Historical scheduler checkpoint

The published baselines remain dirplayer-rs `e82c6992` and childhood-redux
`82c02c5`; identities in the earlier sections are historical.

The approved `commands.rs`-only scheduler repair added a bounded 32-turn batch
for actionless cooperative `Waiting` work while preserving external-action
admission and owner checks. Its five focused scheduler tests passed, and the
locked offline native worker build passed.

The single elevated normal-source run used the existing host runner and the
new receipt [native-cast-scheduler-repair-receipt.json](../../childhood-redux/docs/checkpoints/spybot-native-cast-scheduler-repair-20260919/native-cast-scheduler-repair-receipt.json), with `RUST_BACKTRACE=full` and the trace gate omitted. Discover request 1 succeeded; Start request 2 reached Flash and init, then exposed a `js_sys` imported-static panic at `movie.rs:444` (`get_pref`) through `web_sys::window`. The host transport reported the terminated worker through its 30-second timeout path, with no fallback. The worker was reaped, storage was cleaned, and `worker_alive_after_drop` was false. This scenario result shows the scheduler repair cleared the previous cooperative-turn stall; it is not universal scheduler performance proof. The target-unsafe preference-access repair is the next gate and remains outside this item. Historical receipts remain unchanged.

## Historical native preference checkpoint

The published baselines remain dirplayer-rs `e82c6992`
(`e82c69920e36c970ba20e41bb0b14de16669e832`) and childhood-redux `82c02c5`
(`82c02c52d57fffd9f4af4402043fc76d8a1a9f01`); identities in earlier sections
are historical pre-publication evidence.

The approved native preference repair adds a private player-owned in-memory map
for native `getPref`/`setPref`, clears it during `reset_core`, and shares the
same owner-scoped behavior between global and `_player` forms. Wasm
`localStorage` behavior remains cfg-isolated. The focused ownership regression,
locked offline wasm library check, and locked offline native worker build passed.

The single elevated normal-source run used the existing host runner and the
new receipt [native-preferences-receipt.json](../../childhood-redux/docs/checkpoints/spybot-native-preferences-20260919/native-preferences-receipt.json), with `RUST_BACKTRACE=full` and no trace gate. Discover request 1 succeeded; the first Start result was request 2, a transport timeout after 30 seconds, with no fallback. Teardown attempted Shutdown request 3, received `worker is terminated`, reaped the worker, cleaned storage, and confirmed `worker_alive_after_drop: false`; captured worker stderr was empty. This result removes the prior native `web_sys::window` panic but does not identify a later startup phase or establish a structured next gate. The compact evidence is [diagnosis.md](../../childhood-redux/docs/checkpoints/spybot-native-preferences-20260919/diagnosis.md). Historical receipts remain unchanged.

## Historical native cast-property experiment

Published baselines are dirplayer-rs `e82c6992`
(`e82c69920e36c970ba20e41bb0b14de16669e832`) and childhood-redux `82c02c5`
(`82c02c52d57fffd9f4af4402043fc76d8a1a9f01`); identities above are
historical pre-publication evidence.

The approved `driver.rs` repair adds a synchronous fast path only for
`CastLib.name` and resolved non-Flash `CastMember.name`/`type`. It reuses
`script::get_obj_prop` and the existing `ObjectV4` result application, while
Flash members, JS, FlashObject, and other properties retain their existing
deferred paths. Four focused cast-property tests passed, the existing chained
property async regression passed, and the locked offline native worker build
passed.

The one elevated normal-source run used the existing host runner and receipt
[native-cast-property-fast-path-receipt.json](../../childhood-redux/docs/checkpoints/spybot-native-cast-property-fast-path-20260919/native-cast-property-fast-path-receipt.json),
with `RUST_BACKTRACE=full` and no diagnostic trace gate. Discover request 1
succeeded; the first Start result was request 2, a transport timeout after 30
seconds, with no fallback. Shutdown request 3 observed `worker is terminated`;
the worker was reaped, storage was cleaned, stderr was empty, and
`worker_alive_after_drop` was false. This run produced no structured next gate;
compact evidence is [diagnosis.md](../../childhood-redux/docs/checkpoints/spybot-native-cast-property-fast-path-20260919/diagnosis.md).
Historical receipts remain unchanged.


## Historical native scheduler diagnostic checkpoint

Published baselines are dirplayer-rs `e82c6992`
(`e82c69920e36c970ba20e41bb0b14de16669e832`) and childhood-redux `82c02c5`
(`82c02c52d57fffd9f4af4402043fc76d8a1a9f01`); earlier identities are
historical pre-publication evidence.

The approved native-only diagnostic counted internal completions, external
retains, owner-pump ready sleep/select delays, action admission/completion and
batch resets, plus the direct cast fast path. The locked/offline worker build
passed. The one elevated normal-source run used the existing host runner,
`PARITY_NATIVE_SCHEDULER_DIAGNOSTIC=1`,
`PARITY_EXECUTION_ROUTE=codex-require-escalated`, and `RUST_BACKTRACE=full`;
receipt and compact evidence are retained in
[diagnosis.md](../../childhood-redux/docs/checkpoints/spybot-native-scheduler-diagnostic-20260919/diagnosis.md).

The first Start result was request 2, a transport timeout after 30 seconds.
The only aggregate snapshot was emitted at wall time zero, after one 1-us
cast fast-path call, before any internal completion or delay counter, so it
cannot quantify ready-sleep versus select time. The one retained 10-second
sample showed the owner path and an async-io `kevent` wait chain, but does not
identify the local delay branch or prove a generic scheduler defect. No
scheduler repair or rerun was accepted from this diagnostic. The cast fast
path was later reverted after the generic lifecycle cause was established.
Teardown reaped the worker, cleaned storage,
and confirmed `worker_alive_after_drop: false`; historical receipts remain
unchanged.


## Historical corrected native scheduler diagnostic checkpoint

Published baselines remain dirplayer-rs `e82c6992` and childhood-redux
`82c02c5`; earlier identities and the prior inconclusive scheduler
diagnostic remain historical.

The corrected diagnostic worker build passed. The single elevated normal-source
run used the existing qualified-casts host runner with
`PARITY_NATIVE_SCHEDULER_DIAGNOSTIC=1`, `PARITY_EXECUTION_ROUTE=codex-require-escalated`,
and `RUST_BACKTRACE=full`; compact evidence is retained in
[diagnosis.md](../../childhood-redux/docs/checkpoints/spybot-native-scheduler-diagnostic-corrected-20260919/diagnosis.md).
The first Start result was request 2, a transport timeout after 30 seconds,
with no fallback. Worker PID `88410` was reaped, storage was cleaned, and
`worker_alive_after_drop: false`.

The corrected snapshots reached 128 internal completions and admissions, zero
internal or external retains, zero select-timer wins, and 128 ready-work
sleeps totaling 160.528 ms by wall time 284 ms. The measured 1-ms
ready-work delay therefore consumed 56.5% of the observed startup interval,
with one sleep per completed internal action. The cast fast path consumed
32 us across 5112 calls, but was later reverted after the generic lifecycle
cause was established. This identified the commands.rs ready-work delay as the
smallest generic scheduler repair route at that checkpoint;
external pending semantics remain preserved and untested by this scenario.
The earlier inconclusive checkpoint is unchanged.


## Historical ready-work scheduler repair checkpoint

Published baselines remain dirplayer-rs `e82c6992` and childhood-redux
`82c02c5`; earlier diagnostic identities remain historical.

The approved commands.rs repair replaces the per-cycle ready-work 1ms sleep
with immediate service under a separate 32-cycle `ReadyServiceBatch`. Active
futures remain on the existing select path; synchronous action admission and
completion preserve the batch, while a true external `Retain` remains started
and parked. The focused scheduler suite passed with 7 tests, and the locked
offline native worker build passed.

The one elevated normal-source run used the existing qualified-casts host
runner, the diagnostic gate, and receipt
`docs/checkpoints/spybot-native-scheduler-ready-work-repair-20260919/native-scheduler-ready-work-repair-receipt.json`.
Start request 2 timed out after 30 seconds with no fallback. The worker was
reaped, storage cleaned, and `worker_alive_after_drop: false`. Counters reached
256 internal completions/admissions by wall time 169ms with zero ready sleeps,
zero select-timer wins, and zero Retain results. This accepts the scheduler
throughput gate for the scenario; the next startup gate remains unresolved.
The compact evidence is
[diagnosis.md](../../childhood-redux/docs/checkpoints/spybot-native-scheduler-ready-work-repair-20260919/diagnosis.md).

After acceptance, scheduler counters, driver cast timing, and the temporary
bounded init-op trace were removed. The affected scheduler suite and worker
build were rerun, temporary host logs were removed, and startup was not rerun
for cleanup. Prior diagnostic checkpoints and receipts remain unchanged.

## Accepted historical checkpoint: native notification sink and structured Start

All timeout and terminated-worker sections above are historical checkpoints;
this section records the later accepted sink result.

The causal chain was an unconsumed native notification FIFO: the existing
bounded mailbox retained 256 DTOs, the 257th set native host-event backpressure,
the ignored dispatch result let the normal command loop return, and the ready
Go action remained orphaned. The repair binds an explicit weak,
owner-qualified `NativePlayerNotificationSink` synchronously on `TestPlayer`
before `run_command_loop`. The parity sink acknowledges DTOs in order while
retaining only bounded production metadata; unbound or stale owners still use
the existing bounded mailbox and fail closed at capacity.

The bound command-loop regression, paired unbound overflow regression, and
reentrant sink regression passed. The final offline native-worker build after
the cleanup passed with SHA-256
`6571c3f116fd6f72b14980dbe508fbb8fa747fe72ada9851ebbb308303755136`.
The earlier successful sink-repair receipt used worker
`21eb86eb96a0db9eab3f6a906551ca90071835a2aaef7055a69fb63bd4a4fd44`; the
pre-final-cleanup live receipt used worker
`6c0542ec1b184a42dcad64793532b73575079a4904843de1ab2b0c26e7f6bcdf` and is
retained as historical evidence.

The final single elevated normal-source qualified-casts run used the existing
runner with no diagnostic environment gates. Receipt:
`../../childhood-redux/docs/checkpoints/spybot-native-command-loop-state-corrected-20260920/native-command-loop-clean-integration-final-receipt.json`
(SHA-256
`57633c70faa2a6acee96f47060520bc581e0f2f9c302a0f80adde77b0104ac18`).
Discover request 1 returned capabilities; Start request 2 returned the
structured session `{id: 1, generation: 1}` on the normal source path using
worker `6571c3f116fd6f72b14980dbe508fbb8fa747fe72ada9851ebbb308303755136`.
Shutdown request 3 was acknowledged; stderr was empty, storage was cleaned,
the worker was reaped, and `worker_alive_after_drop` was false. This
establishes the normal source Start result only. Menu/title fidelity, input
behavior, audio, and exact visual comparison remain open.

## Historical Phase 2 source-audio provenance correction

The raw DCR `s.win_flag` member is not IMA ADPCM and is not the exported
browser MP3 fixture. Cast member 5/72 resolves to `ediM` 222707 with 3532 raw
bytes (SHA-256 `e274ab9e1d3f0d04d00cf5e27a37dec72f4cfb199e4cd8f2f158ee195df1c55f`).
Its first four bytes are `00 00 00 00`; a valid chained MP3 stream begins at
offset 4, leaving 3528 bytes (SHA-256 `a1f0b3487362f244531a245ce115f6e5bac9f583536473d92cd248609698c14d`).
The `data_size_field` value 22198 is not an MP3 sample count. The separate
3397-byte SHA-256 `0d24c9d2b7d8df85dbe0dfd41f653add1a2ab41bbae347932108b977ed3d41fe`
file remains the exported/browser control.

The parser correction shares the existing chained-frame validator between
`MediaChunk` classification and `SoundChunk::from_media`, preserves explicit
IMA GUID precedence, trims only the validated prefix, and leaves native
registration on the existing Ruffle MP3 path. Focused parser tests pass (3),
the native actual-member Queue/Play regression passes, and the native audio
subset passes (10). The wasm library check passes; the full wasm package check
still encounters the pre-existing native worker binary cfg boundary. The one elevated combined proof used worker SHA-256
`ed28f904fdf5275d0311235ffcf924f15669c13154c1917c9b36a0177bb6711b`
and receipt
`../../childhood-redux/docs/checkpoints/spybot-native-audio-phase2-20260920/native-audio-phase2-dcr-mp3-combined-receipt.json`.
The t0 checkpoint reached Director ready/frame 3, Flash frame 371, and
asserted frame 371. The first controlled 2.45 s advance stopped at the
structured error `queued member is not a sound`; PCM was 48000 Hz stereo
with 4800 frames at timestamp 0 but remained zero. Shutdown was
acknowledged, storage cleaned, and the worker reaped. Split qualification
was not run after this first gate failure.


## Accepted historical checkpoint: Phase 2 native audio

The earlier `queued member is not a sound` result is historical. Invalid/null
`-1:-1`/`0:0` cast-member sentinels now preserve the browser idle/no-op
contract, while positive unresolved members and corrupt media remain strict
transactional errors.

The locked offline worker build produced SHA-256
`6e5c8b4eb9d81e280498a209c0a5fdc4164deb97ba84a6b39f6bd5ad7e659dd3`. The
combined receipt is
`../../childhood-redux/docs/checkpoints/spybot-native-audio-phase2-20260920/native-audio-phase2-sentinel-combined-receipt.json`;
the split receipt is
`../../childhood-redux/docs/checkpoints/spybot-native-audio-phase2-20260920/native-audio-phase2-sentinel-split-receipt.json`.
Both passed Director frame 6/start and Flash frame 419 with timestamp-0,
48 kHz stereo PCM containing exactly 117600 frames. Both had 112304 nonzero
frames, first nonzero frame 5205, and identical PCM SHA-256
`b21f032f45690576c0630400458b596de9f2c8114b787b23db72c649fb762406`.
Shutdown, storage cleanup, worker reap, and empty stderr were verified for
both.

The 3397-byte MP3 remains the browser/export control. The raw DCR `s.win_flag`
member is the 3532-byte payload (SHA-256 prefix `e274ab`), whose validated
four-byte prefix yields the 3528-byte MP3 stream used by the native path. The
controlled clock and sentinel corrections were accepted. At that checkpoint,
`s.electricity`, rollover, rateShift, input, and exact PCM comparison remained
open. The current promotions and remaining boundary are in the opening
checkpoint above.

The diagnostic Text Xtra fail-closed parser change and four fixtures were
removed. StringChunk-only lookup normalization and owner-local cast-cache
refresh remain provisional and are covered by focused source-shaped tests.
