# Native Director readiness probe receipt

This receipt records the probe-only native route in worktree
`/private/tmp/dirplayer-native-readiness-cumulative-20260918`, branch
`codex/native-dirplayer-readiness`. The accepted combined baseline source is
`7cd3cc78`; baseline evidence is `731b0d0a`. The exact accepted probe source
commit is `42c21c82350590c3a05584c1a643a6b79dc16727`, with tree
`1bd0853ed4c21f92148d5171b5dcb0e901b058b7`. No final evidence commit hash is
recorded yet. The probe changes only the native test registration, disposable
fixture/generator/validator, and this evidence checkpoint. No production
runtime, backend, host, API, or browser build was changed.

## Route and exact symbols

The native route exercised by `test_native_director_probe_shape` is:

1. `vm-rust/tests/e2e/dirplayer_test_movies/mod.rs` registers
   `native_probe`; `native_e2e_test!` in
   `vm-rust/src/player/testing_shared/mod.rs:580-594` creates
   `TestPlayer::new()` and runs it under `run_test`.
2. `vm-rust/src/player/testing.rs:28-42` creates `HarnessRuntime`, an owned
   session/player/owner, and the native command loop. `TestHarness::load_movie`
   at `testing.rs:61-126` reads the primary DCR with `std::fs::read`, derives a
   directory file URL, calls `read_director_file_bytes` at
   `vm-rust/src/director/file.rs:1134-1145`, and mounts it with
   `load_movie_from_dir_owned` at `vm-rust/src/player/mod.rs:7133-7140`.
3. `TestHarness::init_movie` at
   `vm-rust/src/player/testing_shared/mod.rs:281-290` calls
   `run_movie_init_owned` at `vm-rust/src/player/mod.rs:10137-10147`.
   The fixture's four movie handlers set distinct globals, and the probe reads
   `prepareState`, `startState`, `enterState`, and `exitState` independently.
   This demonstrates authored `prepareMovie`, `startMovie`, `enterFrame`, and
   `exitFrame` execution through native startup; it does not infer frame
   advancement from one final state value.
4. `TestPlayer::snapshot_stage` at `vm-rust/src/player/testing.rs:151-163`
   allocates the native bitmap and calls
   `render_stage_to_bitmap` at `vm-rust/src/rendering.rs:450-462`. The
   `StageSnapshot` assertion observes a 32x32 RGBA result with visible pixels.
   `NATIVE_PROBE_PNG`, when set by an evidence run, writes the same PNG to the
   caller-selected path. No absolute evidence path is used by the test.
5. The later `TestPlayer::step_frame` path at `testing.rs:128-149` calls
   `fire_pending_timeouts_owned`, then `run_single_frame_owned`, then sleeps at
   real tempo. The sleep is a recorded harness limitation and remains
   unchanged. The native timeout host adapter returns
   `TimeoutHostDispatch::Unsupported` from
   `vm-rust/src/js_api.rs:5081-5105`; native timeout firing is manually driven
   by `fire_pending_timeouts_owned` at `vm-rust/src/player/mod.rs:12519-12526`.
6. `TestPlayer` teardown is `Drop` at `testing.rs:166-169`, which retires the
   current `HarnessRuntime`. The browser comparison route is
   `BrowserTestPlayer::new` at `vm-rust/src/player/testing_browser.rs:1521-1552`;
   it binds a renderer, loads through
   `load_movie_from_url_owned` at `testing_browser.rs:6960-6991`, drives the
   same owner-bound init/frame functions at `testing_browser.rs:6946-6958` and
   `6993-7018`, and disposes the renderer at `7119-7125`. Native uses a local
   path and CPU bitmap; browser uses an origin URL, browser fetch/host adapters,
   a renderer state, and browser scheduling.

## Fixture boundary

`vm-rust/tests/fixtures/generate_native_director_probe.py` deterministically
generates the parser-valid D5 `native_director_probe.dcr`; the validator checks
its RIFX/MV93, DRCF, KEY*, CAS*/CASt, VWSC, Lctx, Lnam, and Lscr structure.
The score contains one 32x32 QuickDraw shape. The fixture intentionally has no
Flash, audio, fonts, nested movies, external casts, or Xtras. The authored
script records are four distinct handlers, each writing `1` to a distinct
phase global. This is parser-valid fixture and authored startup proof; it is
not evidence that a malformed or rejected fixture is a native host blocker.

The missing-primary test uses the same `TestPlayer::load_movie` entrypoint with
the absent path
`vm-rust/tests/fixtures/native_director_probe_missing_primary.dcr`. Its
expected boundary is the harness's `std::fs::read` panic (`Failed to read ...:
No such file or directory`). It is kept separate from parser and host
classification.

## Evidence and classification

The success test proves native local load, D5 parse, startup script dispatch,
shape score setup, and CPU RGBA capture. It does not prove a native renderer
binding or a completed native frame-advance loop. The separately filtered
step-frame test loads and initializes the same valid fixture, captures once,
then asserts the exact existing boundary `frame execution failed: owned player
has no renderer` from `testing.rs:142`. The immediate source boundary is
`draw_frame_for_owner` at `vm-rust/src/rendering.rs:3890-3903`: the session's
`renderer_state(player_id)` weak binding is absent or cannot be upgraded. That
is a native test-harness presentation boundary, not a production host/runtime
classification.

The browser `DynamicRenderer` path is currently DOM/canvas-bound: the browser
renderer state is created in `vm-rust/src/rendering.rs:3667-3725`, while the
Canvas2D renderer creates `HtmlCanvasElement` instances through
`window().document().create_element("canvas")` at
`rendering.rs:4175-4205`. A proposed native production-host slice would need
to provide an owned native presentation policy/backend, plus a caller-driven
monotonic frame and timeout pump and teardown, around the existing
`RuntimeSession`/player/owner/command-loop and `load_movie`/`init_movie`/
`step_frame` symbols. That is a proposed implementation seam, not a change
demonstrated by this probe.

The statically demonstrated native timeout adapter is separate: native
schedule/clear calls return `TimeoutHostDispatch::Unsupported` at
`vm-rust/src/js_api.rs:5081-5105`, while the fixture-observed blocker is only
the missing renderer binding above. Neither should be conflated with rejected
fixture input or with an unsupported native host conclusion. The next
executable task is to qualify the proposed native presentation/pump seam, then
rerun one bounded `step_frame` and assert frame transition plus per-frame
script state. No such binding or production change is included here.

The later untested route includes native frame advancement after renderer
binding, timeout scheduling beyond the recorded unsupported host adapter, and
all excluded resource families (Flash, audio, fonts, nested movies, external
casts, and Xtras).

## Reproduction

From the worktree:

```sh
mise exec -- python3 vm-rust/tests/fixtures/generate_native_director_probe.py
mise exec -- python3 vm-rust/tests/fixtures/validate_native_director_probe.py

CARGO_TARGET_DIR=/private/tmp/dirplayer-native-readiness-native-stage2-fff7dcc3/target \
  mise exec -- cargo test --manifest-path vm-rust/Cargo.toml --test mod --locked --offline --no-run

NATIVE_PROBE_PNG=/private/tmp/dirplayer-native-readiness-native-stage2-fff7dcc3/evidence/native-probe-stage.png \
CARGO_TARGET_DIR=/private/tmp/dirplayer-native-readiness-native-stage2-fff7dcc3/target \
  perl -e 'alarm 30; exec @ARGV' -- mise exec -- cargo test --manifest-path vm-rust/Cargo.toml --test mod --locked --offline test_native_director_probe_shape -- --nocapture

NATIVE_PROBE_PNG=/private/tmp/dirplayer-native-readiness-native-stage2-fff7dcc3/evidence/native-probe-stage.png \
CARGO_TARGET_DIR=/private/tmp/dirplayer-native-readiness-native-stage2-fff7dcc3/target \
  perl -e 'alarm 30; exec @ARGV' -- mise exec -- cargo test --manifest-path vm-rust/Cargo.toml --test mod --locked --offline test_native_director_probe_shape -- --nocapture

CARGO_TARGET_DIR=/private/tmp/dirplayer-native-readiness-native-stage2-fff7dcc3/target \
  perl -e 'alarm 30; exec @ARGV' -- mise exec -- cargo test --manifest-path vm-rust/Cargo.toml --test mod --locked --offline test_native_director_probe_step_frame_renderer_boundary -- --nocapture

CARGO_TARGET_DIR=/private/tmp/dirplayer-native-readiness-native-stage2-fff7dcc3/target \
  perl -e 'alarm 30; exec @ARGV' -- mise exec -- cargo test --manifest-path vm-rust/Cargo.toml --test mod --locked --offline test_native_director_probe_missing_primary_boundary -- --nocapture
```

The source-fix no-run build and all three bounded test invocations passed. The
fresh normal run reported `1 passed; 0 failed`, `frame=1`, a 32x32 snapshot,
and `phases=prepare,start,enter,exit`. The boundary tests each reported
`1 passed; 0 failed` while asserting their expected panic text. The earlier
two-run evidence remains retained in the preceding success logs.

## Hashes and retained artifacts

The validator reported the parser-valid fixture as 1149 bytes with SHA-256
`ea8ecbe400fee03797fe322fe69814db96cad95e6f7d756365f43babd29d5e9b`.

Retained logs and hashes:

| Artifact | SHA-256 |
| --- | --- |
| `/private/tmp/native-probe-generator-final.log` | `61327ef9fb4211603568985f7fc5a11f827f5d958b0f93d81c6b46ccc3c28aee` |
| `/private/tmp/native-probe-validator-final.log` | `1f86df919613d04d5ee6c76461a8448fd27527e88e4ad52113b170e333731937` |
| `/private/tmp/native-probe-success3.log` | `e39a29f33ecb0570f7b49469ebeca4a16d95daa2f94867261da54fbdd1348f1c` |
| `/private/tmp/native-probe-success4.log` | `d86970aede6bdcc7322ad1ab9e17fdd646e923f1865010a11709d9b8bfd4ebad` |
| `/private/tmp/native-probe-build-source-fix.log` | `2d14cffe3b44785b5fe551181e3e8c15a33109f8f873f19d012360b2a9a9abd8` |
| `/private/tmp/native-probe-success-source-fix.log` | `a8937a8f33add6b4597e9da222917fa1d20ed71e86c1fef404c2ef878571fa9e` |
| `/private/tmp/native-probe-step-source-fix.log` | `37e10451130d24f126ec368366619ae399c4f40d03e5c3565b8715dd2014f8d7` |
| `/private/tmp/native-probe-missing-source-fix.log` | `d5828fad0c3ef43d217566ae403f70bd107ef99f4897e56623b747b9267cc6e2` |
| `/private/tmp/dirplayer-native-readiness-native-stage2-fff7dcc3/evidence/native-probe-stage.png` | `da721b75691d6c62690cdc3dd09a7c63e12b88448465bc3922d5ce1b568cbde34` |
| `/private/tmp/dirplayer-native-probe-evidence-20260918.tar.gz` | `6574f8d0fd5ba804a592aba66bdd2fdb884d6ca15bae9859fc3805d47ac0d6f0` |

The PNG is copied into this checkpoint as
`native-probe-stage.png` with the same SHA-256. The source fixture is retained
in the worktree and its hash is recorded above; the generator and validator
are the reproducible source of that artifact. The archive contains:

```text
native-probe-generator-final.log
native-probe-validator-final.log
native-probe-build-source-fix.log
native-probe-success3.log
native-probe-success4.log
native-probe-success-source-fix.log
native-probe-step-source-fix.log
native-probe-missing-source-fix.log
native-probe-stage.png
```
