# Reboot handoff — Stage 2 paused

The user requested a prompt stop for a laptop reboot. Do not start new work or
validation until the user resumes. The full goal remains implementation through
Stage 2 of `childhood-redux/docs/NATIVE_DIR_PLAYER_PLAN.md`, stopping before
Stage 3. Neither completion nor a blocked goal is claimed.

## Resume here

1. Read this handoff and any pilot notes alongside it. Inspect current Git status,
   branches, remotes and source before relying on historical receipts.
2. Reuse the subagent-pair-program skill. Main agent is navigator; the Work Item
   Lead owns the explicit-owner score startup migration and all Cargo/browser
   runs. A separate fixture pilot owns pointer browser tests. Reestablish writer
   ownership and check for processes before running anything.
3. Complete and review the interrupted score migration and pointer tests before
   building. The working tree is an intermediate implementation, not a verified
   checkpoint. Do not discard unrelated work, run broad formatters, or publish
   unfinished changes.
4. Run the mounted child regression first, then meaningful score/owner/FilmLoop
   tests, native suite, WASM check and actual child/pointer browser fixtures.
   Keep the exact cast/owner/generation checks intact. Continue all remaining
   Stage 2 gates; do not reduce the goal to these current components.

## Published and local work

- `dirplayer-rs`, branch `dev`, origin `https://github.com/iExalt/dirplayer-rs`:
  `fa80cb175625ac0e72022ff399247623f66b00cd` publishes accepted compiled
  backjump accounting and audit receipts.
- `childhood-redux`, branch `main`, origin
  `https://github.com/iExalt/childhood-redux.git`:
  `3d221ba1e30399374652406642991d964312b813` publishes the plan checkpoint.
- Both remote revisions were verified after the user's commit/push request.
  Subsequent changes are local; no further publication requested.
- Existing seven modified Ruffle files must remain unchanged. Ruffle HEAD:
  `79d1ca0f45d79e28c3a0658bbc6b8430d26ef308`.
- `mise.toml` preserved SHA-256:
  `a8d10548ed3a7fbe794a851d546c3367af6f1618e617769f726868c6b086e4e4`.

## Reproduced child failure

The browser adapter omitted named exports required by generated WASM. Re-exporting
real `registerNestedFlashOwner` and `retireNestedFlashOwner` fixed bootstrap;
a transport probe without rebuilding Rust reached both root registrations and
both child startups, then failed child A's first getVariable call with:
`Flash request cast binding is stale or replaced`.

A new native production-startup regression reproduces the problem independently:
parsed frame 0/raw channel 6 has cast `(1,1)`; parent channel 1 remains `(1,91)`
and entered; child 2 is entered but has no member. The child owner is session
79/player 2/generation 1, its route is LocalOwned, and no Flash generation exists.
The exact failure receipt is in `mounted-failure/`. This is one failing regression
with 595 tests filtered out. Earlier 595-pass native evidence is historical;
the augmented current suite is not green.

`Score::begin_sprites` performs Stage member assignment through ambient
`reserve_player_mut` even when its caller selected an explicit child. Other
span eligibility, cast and behavior operations also use ambient access. The
observed child entered=true points directly at member application, not a proved
eligibility failure in this particular regression. Fixture cast offsets are
correct; do not weaken the request fence or switch the active player.

## Approved score migration, interrupted

Core pilot files: `vm-rust/src/player/score.rs`, `mod.rs`, `nested.rs` and in-file
tests. Lead: `/root/stage2_lead`; core: its `flash_fixture_repair` pilot.

Move orchestration to `DirPlayer::begin_score_sprites(ScoreRef, frame, symbols)`.
Use existing get_score/get_score_mut/get_score_sprite_mut selectors and short
borrows: snapshot spans/init records, release the score borrow, apply whole-player
member/behavior/sound effects to the passed player, reacquire the selected sprite.
No mutable sprite borrow may span a whole-player call. No dummy/temporarily
removed score, unsafe-pointer adapter or ambient wrapper is acceptable.

Convert create_behavior and every ambient operation reachable from owned startup,
including exit cleanup, eligibility, Stage member setter, cast/bitmap queries,
behavior attachment, frame-script lifecycle, sound and recursive FilmLoop startup.
Preserve Stage setter side effects and FilmLoop relative-cast/member semantics.
Update begin_all_sprites and direct FilmLoop advancement call sites after releasing
member borrows; this covers both initial-load and post-prepare initialization.

Final stopped state: a provisional explicit DirPlayer entry and create_behavior_owned
exist, and the old Score::begin_sprites body was removed. Borrow-safe conversion,
recursive FilmLoop calls, transitive ambient audit and callsite/test repair remain
incomplete. See score-pilot.md for exact details. No compilation was attempted
for this migration; do not treat the saved source as buildable.

Required tests: reproduced mounted case, independent sessions with colliding child
IDs and different parent state, Stage member side effects, FilmLoop relative cast
and behavior attachment, then native/WASM/actual-browser integration.

## Pointer tests and fixtures, interrupted

Fixture pilot: `/root/reference_audit`. Exclusive files released by lead:
`vm-rust/src/player/testing_browser.rs`, `tests/e2e/dirplayer_test_movies/nested.rs`,
and `handlers/datum_handlers/flash_object.rs` for decoder visibility only.

Six new fixture artifacts are integrated under `vm-rust/tests/fixtures`:
`generate_flash_pointer_fixture.py`, `validate_flash_pointer_fixture.py`,
`flash_pointer_fixture_provenance.md`, and `flash_mouse_{a,b,reentry}.swf`.
The navigator inspected bytecode against actual Ruffle source, regenerated bytes
independently, and ran structural plus negative encoding checks. Runtime not proved.

- A SHA-256: `2112fdcd4153807408c02d3b72aeae08ca376787ca8a5a32aa98f5f6edf9c44d`
- B: `60c316ccea5c30301d6244d3123331b583bde6451498dedcd71897cff982f118`
- Reentry: `26ecc422a5ad7d7ac7a392a58e4e9f61ef2f1ed67c9bb45b1d56dd27c030f268`

Corrected fixture defects: DefineFunction ActionLength excludes body; onMouseMove
belongs on root MovieClip, onPress on button; Stop prevents timeline reinitializing
counters. Negative CallMethod test must mutate decoded opcode, not ASCII R (0x52)
in the callback name. Geometry is 550x400, button click at 250,200.

Approved browser work:
- Make real decode_mouse_dispatch_response pub(crate), then test actual JsValue
  envelopes including invalid owner/generation, host errors and malformed values.
- Two actual BrowserHandles and production root Flash registration; actual SWFs;
  zero initial globals, move-before-press/order, independent owners, reset/stale
  rejection/recreation. Use commands::run_player_command with explicit selected
  session/id/owner and inspect CommandTurn::Complete result, not click's discarded
  result or self.harness_runtime's unrelated default owner.
- Generalize register_browser_handle_flash_owner to capture actual player.queue_tx
  rather than its isolated forgotten receiver. BrowserHandle constructor already
  starts the production command loop.
- Real ExternalInterface reentry: separate fresh positive-control and armed Ruffle
  instances (a previously pressed button would make a second click a false negative).
  Move hook synchronously resets captured BrowserHandle; press hook increments an
  external counter. Require positive control, reset success, typed stale command
  error, no press-counter increment and independent B still functioning. Never
  read retired A globals as proof of absence. No route mocks/production fault hooks.
- Release every handle/session borrow before await or host calls. Restore globals
  with RAII before test closures drop, including early-error cleanup.

## Validation and remaining goal

Use mise; Cargo locked/offline, CARGO_INCREMENTAL=0 and shared target
`/private/tmp/dirplayer-parser-review-build/target` if it survives reboot. Do not
assume temporary binaries/logs survive. No copied build caches, no broad rustfmt.
Network/server/elevated tools run outside sandbox. One Cargo/browser monitor.

Latest pre-migration checkpoints: 595 native tests passed; WASM check passed;
frontend manager 10 tests and lifecycle 38 passed. Child browser failed as above.
Current newly added regression fails, and interrupted migrations are unverified.
Detailed historical context is in `prior-runtime-checkpoint.json` and existing
flash-nested-host/mouse-down/interpreter audit documents.

Stage 2.1 and bounded parts of 2.2/2.3/2.6/2.8 are accepted at recorded revisions.
Production compiled execution/debugger, all scheduler producers, other browser
routes/timers, extension/resource closure, Ruffle callback-origin ownership,
remaining player/render/audio/cache globals and final one-checkout cutover remain
open. Never mark full Stage 2 complete from a focused green suite.

## Final stop confirmation

Both pilots stopped after saving in-flight writes. The Work Item Lead checked
processes outside the sandbox and found no task Cargo, VM-test, Playwright,
browser-runner or webpack process. An unrelated LM Studio worker was left alone.
No new build, browser run, commit or push was started for this reboot request.
The local work is incomplete and compile-unverified.

Pilot details: `score-pilot.md` and `pointer-pilot.md`. Current source identities
and repository status are recorded in `worktree-manifest.json` and
`worktree-status.txt`; source files themselves remain in their original locations.
