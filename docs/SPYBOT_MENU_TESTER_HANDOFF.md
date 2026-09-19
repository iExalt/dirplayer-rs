# Spybot native P0 handoff

Checkpoint: 2026-09-19. No commit, push, or staging was performed.

## Navigator resume brief

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

## Repository identities and status

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

## First-blocker next contract

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

The current menu status remains blocked at the stack-localized `foreign or stale
DatumRef` panic at `CastHandlers::cast_lib`. Metal construction progressed and
the tracked GPU discovery route is accepted as an execution prerequisite; it
does not qualify Flash behavior. The production Flash scaffold remains
unqualified for frame 371, frame 419, the
`introTitleReady` bridge, and settled Director `start`; those remain later
qualification gates after the explicit-context cast ownership repair.

## Publication checkpoint

- [x] P0 structured external-cast blocker retained with teardown evidence.
- [x] Five local shell aliases and hashes audited; title startup route qualified
  through existing owner/capability/session orchestration.
- [x] Current menu status recorded as blocked by the stack-localized `foreign or
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
