# Spybot native P0 handoff

Original checkpoint: 2026-09-19. The integrated current status is below.

## Current checkpoint (2026-09-21)

The published baselines are dirplayer-rs `3e44c1c3` and childhood-redux
`01212bff`; the active nested Ruffle used by the current native visual recipe is
`fd5d8dda`. The normal source path reaches Flash 371, advances through 419,
dispatches the owned `introTitleReady` callback, and settles Director at
`start`. Accepted native capabilities now include owned state inspection and
typed invocation, deterministic advancement, RGBA and cumulative 48 kHz stereo
PCM capture, the fixed presentation viewport, qualified external casts and
preferences, Flash seek/advance/callback handling, Director rateShift, and
menu pointer move/down/up with capture and cancellation. Available-DCR rollover
silence is qualified by recovered source plus the browser handler comparison.

The user-selected visual contract uses pinned native Ruffle source artwork for
frames 371–419 while Bevy keeps independent scheduling, input, and state;
representative accepted native/Bevy pixels are exact. Native Director audio now
uses the production Bevy/Rodio 0.22.2 decode/resample/mix primitives while
retaining independent source scheduling, channels, queueing, gain, pan,
rateShift, ownership, and timestamps. The normal-source 2.45-second capture is
exactly equal to Bevy: 117,600 stereo frames, first active frame 2401, SHA-256
`422c4f9dc76d58af3b11c8d360f735da8daa06d009209e766e626792273851d5`.
All four isolated source cues match the Bevy/Rodio reference, and source
`s.select` rateShift -2 is split/combined deterministic. The complete repeated
comparison campaign has not run. All later checkpoint sections are historical
evidence unless this section explicitly promotes their result; the experimental
cast-property fast path was reverted.

The next boundary is the full normal-source comparison campaign through START,
including real-input select/begin PCM, repeated exact visual/state checkpoints,
and clean teardown. Direct-title or frame-setting routes remain diagnostic-only
and cannot replace the normal source acceptance path. Embedded Flash remains on
the pinned Ruffle path. The accepted audio receipt is
[native-audio-phase2-clean-final-receipt.json](../../childhood-redux/docs/checkpoints/spybot-native-audio-phase2-20260920/native-audio-phase2-clean-final-receipt.json).

## Historical P0 navigator brief

The user authorized the bounded native implementation and one normal-source
P0 process. The probe has run exactly once and stopped at its first structured
blocker. Preserve the existing native WIP and repository-local dirty entries.
Do not rerun P0 until a separately approved component contract resolves or
qualifies the blocker.

The source/native menu boundary remains exact evidence through START activation.
A blocked probe is accepted evidence and is not menu completion. Keep the
one-scenario-per-process boundary, original DCR, existing guard, and explicit
teardown proof. Never use a direct-title/frame-419 shortcut or fallback request
to replace the normal source path.

## Historical repository identities and status

- dirplayer-rs: `/Users/clliaw/Projects/dirplayer-rs`, branch `dev`,
  origin `https://github.com/iExalt/dirplayer-rs`, HEAD
  `2116077c1a5c0d1de896c3863ba01feef0a4a88f`.
- childhood-redux: `/Users/clliaw/Projects/childhood-redux`, branch `main`,
  origin `https://github.com/iExalt/childhood-redux.git`, HEAD
  `0b2f7d71f0e9480e4fe9c4faee9f13532944e670`.
- Nested Ruffle: `2dfcfedb612509e5d31e7b6e710194b8d3e5e208`, branch
  `feat/native-parity`.
- DCR:
  `resources/spybot/spybot-nightfall-incident.dcr`,
  SHA-256
  `ddf24b667a8d014856d9db42e1658cbf9714847f1cadc2c2c14d21f8e5950c77`.

The receipt preserves complete porcelain entries, including status codes, for
both repositories. It also records all dependency-closure hashes, the harness
hash, worker hash, toolchain, exact envelopes, and the typed transport limit.

## Owned changes

- `/Users/clliaw/Projects/childhood-redux/workspace/parity/examples/native_dirplayer_spybot_p0.rs`
  now uses typed `ProcessConfig`/`ProcessWorker`/`SessionClient`,
  independently validates the two checkout/resource roots, preserves full
  status entries, records provenance, and performs cleanup after Discover or
  Start failures.
- `/Users/clliaw/Projects/childhood-redux/docs/checkpoints/spybot-native-p0-20260919/native-start-receipt.json`
  records the one P0 run and its result.
- `/Users/clliaw/Projects/dirplayer-rs/docs/SPYBOT_MENU_TESTER_SPIKE.md`
  records actual authorization, evidence, the transient Flash frame-419
  `title` versus settled Director `start`, and the historical silent
  rollover qualification requirement.
- This handoff records the current checkpoint.

No runtime, guard, Cargo, or unrelated WIP file was edited by this item.

## Focused validation

- `mise exec -- cargo check -p parity --example native_dirplayer_spybot_p0 --locked --offline`
  passed.
- `mise exec -- rustfmt --edition 2024 --check workspace/parity/examples/native_dirplayer_spybot_p0.rs`
  passed.
- The P0 command was run exactly once:
  `mise exec -- cargo run -p parity --example native_dirplayer_spybot_p0 --locked --offline`,
  with the two explicit environment paths in the receipt and spike document.
- The receipt JSON validates and is the sole process-run evidence for this
  checkpoint.

## P0 result

Discover request id 1 returned capabilities. Start request id 2 used protocol
version 1 and the exact normal source configuration. The first structured
result was:

```text
code: unsupported
native single-dcr worker does not support external casts:
sound_level_1.cst, sound_level_2.cst, sound_level_3.cst,
sound_level_4.cst, sound_level_5.cst
```

The receipt retains the complete source paths from the worker response. No
fallback or direct-title request was sent.

Shutdown request id 3 returned `acknowledged`. ProcessWorker termination
returned `reaped`; storage was cleaned, stderr was empty, and the worker PID
was not alive after drop. The typed transport validates response protocol version
and request id but does not expose raw response envelopes; that limitation is
recorded rather than inferred around.

## Qualified-cast probe result

A separately built latest native worker was exercised by the new qualified
probe after the P0 harness and receipt were hash-checked. The probe sent
exactly Discover, one normal source Start, and an attempted Shutdown with the
five audited `resource_aliases` entries. Its receipt is
[native-qualified-casts-receipt.json](../../childhood-redux/docs/checkpoints/spybot-native-qualified-casts-20260919/native-qualified-casts-receipt.json);
the discovery analysis is
[next-gate-analysis.md](../../childhood-redux/docs/checkpoints/spybot-native-qualified-casts-20260919/next-gate-analysis.md).

The aliases passed far enough to enter ordinary DCR cast loading, but the
receipt does not prove all five were applied. The first Start result was a
transport error from a non-unwinding native abort in
`wasm_bindgen::JsValue::from` at `cast_member.rs:4451`, inside
`try_parse_vector_shape`. It occurred before the anticipated embedded Flash
guard could return a structured result. Shutdown returned `worker is
terminated`; `terminate` still reported `reaped`, storage was cleaned, and the
PID was dead. The original P0 harness and receipt remain byte-identical.

The accepted route demonstrates source-root aliases, hashes, root confinement,
external shell qualification, and owner-bound startup orchestration. The
remaining ownership concern is the legacy `CastHandlers::cast_lib` resolver
versus an explicit `RuntimeSession` context. The next proposed component is a
typed explicit-context cast handler repair; this is not yet a production
qualification result.

## Qualified probe validation

- `mise exec -- cargo build --locked --offline --bin native_dirplayer_parity_worker`
  passed in `dirplayer-rs/vm-rust` before the probe.
- `mise exec -- rustfmt --edition 2024 --check workspace/parity/examples/native_dirplayer_spybot_qualified_casts.rs`
  passed.
- `mise exec -- cargo check --locked --offline -p parity --example native_dirplayer_spybot_qualified_casts`
  passed.
- The qualified probe ran once with the explicit source-root and worker
  environment paths. Its receipt validated with
  `mise exec -- python -m json.tool`.
- `git diff --check` passed for the updated evidence documents; no runtime
  file was edited by this probe and no stage, commit, or push was performed.

## Historical first-blocker contract

The production native Flash scaffold remains explicitly unqualified. The
latest elevated Metal-capable run cleared the renderer gate, reached the
qualified route, and localized the current menu result to a stack-localized
`foreign or stale DatumRef` panic at `CastHandlers::cast_lib`.
The next proposed implementation is the explicit-context cast handler repair
described above, preserving owner/capability isolation and existing shell
qualification. It must not add a fallback, direct-title request, PCM work,
destination behavior, Flash implementation, or broad refactor.

After that contract is approved and verified, rebuild the worker and obtain a
new approval before any further normal-source P0 run.

## Parser-repair rerun result

The approved narrow repair replaced the vector-shape success log with
target-safe `debug!` logging and added native valid/malformed parser tests.
Both focused tests passed; the locked offline worker build and installed
`wasm32-unknown-unknown` library check passed. The four historical harness and
receipt identities remained byte-identical.

A separate rerun harness supplied the same five aliases and sent exactly
Discover request 1, normal source Start request 2, and Shutdown request 3.
The new receipt is
[native-qualified-casts-rerun-receipt.json](../../childhood-redux/docs/checkpoints/spybot-native-qualified-casts-rerun-20260919/native-qualified-casts-rerun-receipt.json).
The first Start result was structured `runtime` with the exact blocker:
`native Ruffle offscreen renderer failed: Ruffle does not support OpenGL on
macOS/iOS.` Shutdown was acknowledged, termination reported `reaped`, storage
was cleaned, stderr was empty, and the worker PID was dead. No fallback was
sent.

At that checkpoint the menu remained blocked at the stack-localized `foreign or stale
DatumRef` panic at `CastHandlers::cast_lib`. Metal construction progressed and
the tracked GPU discovery route is accepted as an execution prerequisite; it
does not qualify Flash behavior. The production Flash scaffold remains
unqualified for frame 371, frame 419, the
`introTitleReady` bridge, and settled Director `start`; those remain later
qualification gates after the explicit-context cast ownership repair.

## Historical publication checkpoint

- [x] P0 structured external-cast blocker retained with teardown evidence.
- [x] Five local shell aliases and hashes audited; title startup route qualified
  through existing owner/capability/session orchestration.
- [x] That checkpoint recorded the stack-localized `foreign or
  stale DatumRef` panic at `CastHandlers::cast_lib`; Metal route accepted and
  production Flash scaffold remains unqualified.
- [ ] Repair and review the explicit `RuntimeSession`-context cast handler.
- [ ] Requalify native Flash frame progression and `introTitleReady` only after
  the ownership repair and an approved runtime route.

## Proposed staging closure

`dirplayer-rs` paths essential to the adopted native work and evidence:

- `.gitmodules`
- `ruffle`
- `vm-rust/Cargo.toml`
- `vm-rust/Cargo.lock`
- `vm-rust/src/lib.rs`
- `vm-rust/src/native_flash.rs`
- `vm-rust/src/native_parity_worker.rs`
- `vm-rust/src/player/cast_member.rs`
- `vm-rust/src/player/mod.rs`
- `vm-rust/src/player/session.rs`
- `vm-rust/src/player/testing.rs`
- `docs/SPYBOT_MENU_TESTER_SPIKE.md`
- `docs/SPYBOT_MENU_TESTER_HANDOFF.md`
- `docs/SPYBOT_EXTERNAL_CAST_AUDIT.md`
- `docs/SPYBOT_NATIVE_GPU_DISCOVERY.md`

`childhood-redux` paths essential to the accepted native probes and receipts:

- `workspace/parity/examples/native_dirplayer_spybot_p0.rs`
- `workspace/parity/examples/native_dirplayer_spybot_qualified_casts.rs`
- `workspace/parity/examples/native_dirplayer_spybot_qualified_casts_host_run.rs`
- `workspace/parity/examples/native_dirplayer_spybot_qualified_casts_rerun.rs`
- `docs/checkpoints/spybot-native-p0-20260919/native-start-receipt.json`
- `docs/checkpoints/spybot-native-qualified-casts-20260919/native-qualified-casts-receipt.json`
- `docs/checkpoints/spybot-native-qualified-casts-20260919/next-gate-analysis.md`
- `docs/checkpoints/spybot-native-qualified-casts-rerun-20260919/native-qualified-casts-rerun-receipt.json`
- `docs/checkpoints/spybot-native-qualified-casts-host-20260919/native-qualified-casts-host-receipt.json`
- `docs/checkpoints/spybot-native-qualified-casts-host-20260919/native-qualified-casts-host-backtrace-receipt.json`

Exclude only `.DS_Store`, `.cache`, build outputs, and unrelated WIP. Do not
stage or commit as part of this checkpoint.

## Historical explicit-context checkpoint

Published baselines for this checkpoint are dirplayer-rs `e82c6992`
(`e82c69920e36c970ba20e41bb0b14de16669e832`) and childhood-redux `82c02c5`
(`82c02c52d57fffd9f4af4402043fc76d8a1a9f01`); older identities above are
historical pre-publication evidence.

The approved 2026-09-19 repair is implemented in
`vm-rust/src/player/handlers/cast.rs` and
`vm-rust/src/player/handlers/manager.rs`. Both cast handlers now use the
manager's explicit `ExecutionContext`, while allocator owner and generation
guards remain unchanged. The focused player-2 ownership regression, manager
ownership tests, existing `cast_lib` guard tests, and locked offline native
worker build passed.

The single approved elevated normal-source run used `RUST_BACKTRACE=full` and
the proposed receipt path
`docs/checkpoints/spybot-native-explicit-cast-context-20260919/native-explicit-cast-context-receipt.json`.
It recorded Discover request 1 and exactly one normal-source Start request 2.
The first Start result was a transport timeout after 30 seconds, with no
fallback request. Teardown attempted Shutdown request 3, received `worker is
terminated`, reaped the worker, cleaned storage, captured empty stderr, and
confirmed `worker_alive_after_drop: false`. Historical receipts and
pre-publication identities remain unchanged.

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

Published baselines are dirplayer-rs `e82c6992`
(`e82c69920e36c970ba20e41bb0b14de16669e832`) and childhood-redux `82c02c5`
(`82c02c52d57fffd9f4af4402043fc76d8a1a9f01`); older identities above are
historical pre-publication evidence.

The approved native preference repair adds a private player-owned in-memory map
for native `getPref`/`setPref`, clears it during `reset_core`, and keeps the
global and `_player` forms interoperable only within that owner. Wasm
`localStorage` behavior remains cfg-isolated. The focused ownership regression,
locked offline wasm library check, and locked offline native worker build passed.

The one elevated normal-source run used the existing host runner and the new
receipt [native-preferences-receipt.json](../../childhood-redux/docs/checkpoints/spybot-native-preferences-20260919/native-preferences-receipt.json), with `RUST_BACKTRACE=full` and no trace gate. Discover request 1 succeeded; the first Start result was request 2, a transport timeout after 30 seconds, with no fallback. Teardown attempted Shutdown request 3, received `worker is terminated`, reaped the worker, cleaned storage, and confirmed `worker_alive_after_drop: false`; captured worker stderr was empty. The native `web_sys::window` panic no longer appeared, but this result does not identify a later startup phase or establish a structured next gate. Compact evidence is retained in [diagnosis.md](../../childhood-redux/docs/checkpoints/spybot-native-preferences-20260919/diagnosis.md). Historical receipts remain unchanged.

## Historical native cast-property experiment

Published baselines are dirplayer-rs `e82c6992`
(`e82c69920e36c970ba20e41bb0b14de16669e832`) and childhood-redux `82c02c5`
(`82c02c52d57fffd9f4af4402043fc76d8a1a9f01`); identities above are
historical pre-publication evidence.

The approved driver-only cast-property fast path is limited to
`CastLib.name` and resolved non-Flash `CastMember.name`/`type`, reusing the
existing getter and result application. Focused equivalence, owner/error, and
deferred receiver tests passed, as did the locked offline native worker build.

The one elevated normal-source run used the existing runner and receipt
`docs/checkpoints/spybot-native-cast-property-fast-path-20260919/native-cast-property-fast-path-receipt.json`,
with `RUST_BACKTRACE=full` and the trace gate omitted. Discover request 1
succeeded; the first Start result was request 2, a transport timeout after 30
seconds, with no fallback. Shutdown request 3 observed `worker is terminated`;
the worker was reaped, storage cleaned, stderr empty, and
`worker_alive_after_drop: false`. The run did not produce a structured next
gate. Historical receipts and identities remain unchanged.


## Historical native scheduler diagnostic checkpoint

Published baselines are dirplayer-rs `e82c6992` and childhood-redux `82c02c5`;
identities in earlier sections are historical pre-publication evidence.

The bounded native scheduler diagnostic and locked/offline worker build passed.
The one elevated normal-source run used the existing qualified-casts host
runner with `PARITY_NATIVE_SCHEDULER_DIAGNOSTIC=1`,
`PARITY_EXECUTION_ROUTE=codex-require-escalated`, and `RUST_BACKTRACE=full`.
Its receipt and compact evidence are retained in
`docs/checkpoints/spybot-native-scheduler-diagnostic-20260919/diagnosis.md`.

The first Start result was request 2, a transport timeout after 30 seconds.
The sole aggregate snapshot occurred at wall time zero after one 1-us cast
fast-path call and before any internal completion or delay totals, so it cannot
separate the owner-pump 1-ms ready sleep from the select timer. The retained
10-second sample confirms owner-pump/action activity and a dominant async-io
wait stack, but does not prove a generic scheduler defect or cast fast-path
cost. No scheduler repair or rerun followed from this evidence. The specialized
fast path was later reverted after the generic lifecycle cause was established.
Teardown reaped the worker, cleaned storage, and confirmed
`worker_alive_after_drop: false`; historical receipts remain unchanged.


## Historical corrected native scheduler diagnostic checkpoint

Published baselines remain dirplayer-rs `e82c6992` and childhood-redux
`82c02c5`; earlier identities and the prior inconclusive diagnostic are
historical.

The corrected native diagnostic build passed. The one elevated normal-source
run used the existing qualified-casts host runner with
`PARITY_NATIVE_SCHEDULER_DIAGNOSTIC=1`,
`PARITY_EXECUTION_ROUTE=codex-require-escalated`, and `RUST_BACKTRACE=full`.
Compact evidence is retained in
`docs/checkpoints/spybot-native-scheduler-diagnostic-corrected-20260919/diagnosis.md`.
Start request 2 timed out after 30 seconds with no fallback. Worker PID
`88410` was reaped, storage cleaned, and `worker_alive_after_drop: false`.

The corrected counters recorded 128 internal completions and admissions, zero
internal/external retains, zero select-timer wins, and 128 ready-work sleeps
totaling 160.528 ms by wall time 284 ms. The owner pump therefore spent 56.5%
of the observed interval in the existing 1-ms ready-work delay, one sleep per
completed internal action. Cast fast-path cost was 32 us across 5112 calls,
but the fast path was later reverted after the generic lifecycle cause was
established. The smallest source-backed next route at that checkpoint was a
commands.rs scheduler repair preserving bounded fairness and real external
pending waits. The
earlier inconclusive checkpoint and receipt remain unchanged.


## Historical ready-work scheduler repair checkpoint

Published baselines remain dirplayer-rs `e82c6992` and childhood-redux
`82c02c5`; earlier diagnostic identities remain historical.

The commands.rs scheduler repair now services ready cooperative work
immediately under a separate 32-cycle batch. Active futures retain the existing
select path, and external `Retain` actions remain started and parked. The
focused scheduler suite passed 7 tests and the locked offline native worker
build passed.

The one elevated normal-source run used the existing qualified-casts runner,
the diagnostic gate, and
`docs/checkpoints/spybot-native-scheduler-ready-work-repair-20260919/native-scheduler-ready-work-repair-receipt.json`.
Start request 2 timed out after 30 seconds with no fallback. Teardown reaped
the worker, cleaned storage, and confirmed `worker_alive_after_drop: false`.
The counters reached 256 internal completions/admissions by 169ms with zero
ready sleeps, zero select-timer wins, and zero Retain results. This accepts the
scenario scheduler gate but leaves the next startup result unresolved.
Evidence is in
`docs/checkpoints/spybot-native-scheduler-ready-work-repair-20260919/diagnosis.md`.

Diagnostic counters, driver cast timing, and the temporary bounded init-op
trace were removed after the causal lifecycle repair. The affected scheduler suite and worker build
were rerun, temporary logs removed, and no cleanup startup rerun was made.
Earlier diagnostic receipts remain unchanged.

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
