# Native DirPlayer implementation status

This checklist follows the [native DirPlayer roadmap](../../childhood-redux/docs/NATIVE_DIR_PLAYER_PLAN.md).
The roadmap owns architecture and milestones; this file retains detailed task and
verification history. Baseline `297da4a`; the implementation checkpoint developed
on `native-bevy` is preserved on `dev` over upstream main plus the audio
fix. Current Stage 1 source is `dev` at
`65772143f59e18b239ea34a86affd4f77a5092cf`. **Stages 2–6 runtime implementation
remain paused.** Publication is not integration or
acceptance of the review-stage changes. See the
[publication checkpoint](checkpoints/native-ownership-20260917/README.md) for
compact patches and retained verification records.

## Current Stage 1 checkpoint

The authorized work is complete and has stopped after Stage 1. Existing runtime source, dirty Ruffle
integration and archived review patches remain unchanged by this work.

- [x] **1.1:** Accept the scoped baseline with retained revisions, licenses,
  toolchains and fixture availability in the
  [baseline manifest](checkpoints/stage1-baseline/manifest.json). Fresh
  source-isolated `297da4a` browser playback reached Spybot frame 6 and passed
  all eight audio cases without script errors; see the
  [fresh result](checkpoints/stage1-baseline/audio-reproduced-297da4a.json) and
  [exact reproduction/build record](checkpoints/stage1-baseline/reproduction-run.json).
  The [baseline document](NATIVE_DIR_PLAYER_STAGE1_BASELINE.md) separates this
  reproduction from historical shapes 1/0, whose exact source manifest is not
  retained and whose fixture is currently missing. The full licensed/Habbo
  campaign is excluded. Neither historical nor freshly reproduced historical
  source results certify the current `dev` runtime.
- [x] **1.2:** Refresh and classify 1,311 lexical findings across 1,042 source
  files (900 Rust, 142 JS/TS), including 209 Ruffle findings. Every finding has
  an explicit owner or reviewed narrow disposition in the
  [classification manifest](native-ownership-classification.json); the
  [inventory](NATIVE_OWNERSHIP_INVENTORY.md) retains manual supplemental review
  and lexical limitations.
- [x] Verify deterministic scanner output and focused scanner/checker
  regressions. Retained receipts: [scanner summary](checkpoints/stage1-inventory/summary.json),
  [source coverage](checkpoints/stage1-inventory/coverage.json),
  [scanner tests](checkpoints/stage1-inventory/tests.log),
  [classification tests](checkpoints/stage1-inventory/classification-tests.log),
  and [classification gate](checkpoints/stage1-inventory/classification-check.log).

Stages 1.1 and 1.2 are accepted within those explicit coverage boundaries.
Stages 2–6 remain paused; no runtime migration or archived patch integration
was performed as part of Stage 1.

Use `mise run check:ownership-inventory`, `mise run test:ownership-audit`,
`mise run test:ownership-inventory` and `mise run test:baseline-inputs` for the
Stage 1 gates. The existing `check:ownership` task is the separate Stage 2
removal gate; unresolved globals, TLS and legacy access paths remain.

## Retained runtime checkpoint

The latest reviewed combined source is in
`/private/tmp/dirplayer-combined-reviewed-20260916`. Its
`integration-evidence/flash-verified-pause-20260917.json` records:

- [x] Native test compilation: zero errors; exact library binary **480 passed, 0 failed**.
- [x] WASM test compilation: zero errors.
- [x] Actual browser runtime: **13 fixtures passed, 0 failed**, including Flash evaluator binding/replacement.
- [x] Frontend Flash lifecycle: **34 passed, 0 failed**; targeted generated-capability TypeScript check passed.
- [x] Six recorded checkpoint source hashes and the 408-file live baseline rechecked during roadmap revision.
- [ ] Integrate the latest combined review batch into the live checkout and verify that resulting source.

Native gate: `native-tests-review-20260917T021707569805Z`; WASM gate:
`wasm-tests-review-20260917T021822298126Z`; browser gate:
`browser-isolation-runtime-20260917T021843Z`. Their evidence is under the review
stage's `.cache/native-bevy/evidence/`. These are retained scoped results, not
new test executions or full movie-campaign acceptance.

Live remains at the accepted SysMenu/reset integration boundary:
`.cache/native-bevy/evidence/sysmenu-reset-integration-20260916T191307Z/`.
The latest Flash, scheduler, host-lifecycle and native snapshot work includes
changes verified only in the review stage. Ruffle callback-origin work remains
an isolated, uncompiled nine-file proposal. JS-Lingo legacy TLS and global player/
renderer paths remain. Passing native library tests is not a working native player.

## Current remaining sequence

Milestone IDs refer to the governing roadmap. Each milestone tracks implementation,
verification and integration separately; its complete exit gate controls acceptance.

- [ ] **2.10, early consolidation:** preserve compact evidence, reconcile current bases, integrate the reviewed combined batch and verify live source.
- [ ] **2.6–2.8:** close initial Flash access before reservation, actual Ruffle callback-origin routing, and JS-Lingo runtime/object/RNG ownership.
- [ ] **2.9–2.10:** finish player graph, render/audio effects, loading/cache ownership and remove temporary adapters in one verified checkout.
- [ ] **3.1–3.2:** accept interleaved/separate-thread isolation, lifecycle/panic cleanup, static audit and applicable Miri.
- [ ] **4.1–4.5:** implement native services, exact advancement, RGBA, PCM and the Director-only executable slice.
- [ ] **5.1–5.4:** integrate native Ruffle/Bevy and accept Spybot title, START, tower 3 and a legal gameplay sequence.
- [ ] **6.1–6.3:** deliver the parity worker/backend and certify repeatability, cleanup, performance, fidelity and provenance.

Stage 1 baseline and inventory are complete within their recorded scope.
The complete licensed browser campaign is not verified; the broader Habbo test
lacks its required movie. Native Xtras, Shockwave 3D, native nested Director,
full campaign and Linux/Windows certification remain deferred as specified in
the roadmap, while existing browser behavior must be preserved.

Use one Cargo target, `/private/tmp/dirplayer-parser-review-build/target`, with
`CARGO_INCREMENTAL=0`. Do not copy dependency/build trees. Retain compact
commands/logs/manifests and reviewed source deltas before removing obsolete
artifacts. The sequence above remains paused beyond the authorized Stage 1 work.

## Historical checkpoints — not the current to-do list

Everything below is retained verbatim from the preceding implementation log.
Headings such as “Current”, “Active” and “Pending”, unchecked tasks, running-job
statements and compiler failures describe their historical checkpoint only.
Later entries can supersede them. Consult the Stage 1 checkpoint, retained runtime checkpoint and
remaining sequence above for present status; no old green subset proves a full
milestone. The original Stage 1 checkmark denotes its scoped baseline only.

## Current acceptance sequence

The user updated the goal: resolve compiler errors first, then ensure tests pass.
The earlier stop-at-clean-compile instruction is superseded. Historical entries
that say runtime tests were unrun remain evidence of those earlier checks only.

- [x] Compile native and WASM test targets and run focused runtime gates.
  The integrated FileIO/XML batch passes 436 native library tests (0 failed,
  0 ignored) and four real browser fixtures: handle isolation, nested input,
  Multiuser socket lifecycle, and remote FileIO reads. Final source snapshots
  were unchanged during verification.
- [x] Integrate 18 reviewed FileIO/XML files with exact before/after hashes and
  rollback source archive in
  `.cache/native-bevy/evidence/fileio-xml-integration-20260916T183625Z/`.
  FileIO/XML state is player-owned; pending network requests capture inputs
  and execute outside the VM borrow. Follow-up review found that the network
  reset helper is tested directly but not yet called by the actual session
  reset path; full reset isolation remains open pending the wiring fix below.
- [x] Complete the post-integration production gate
  `production-owner-batch-20260916T183645Z`: WASM, TypeScript, frontend,
  Flash-owner tests, bridge-owner tests, and VM-owner assertions all exit 0.
  Source hashes were unchanged; all 18 integrated source hashes were reverified.
- [ ] Complete Stage 2 and full-plan verification. Owned playback, direct host
  sinks, and SysMenu migration remain under implementation/review. The broader
  Habbo integration is still unverified because the required movie is absent.

Use `/private/tmp/dirplayer-parser-review-build/target` for Cargo verification.
Do not create per-stage target directories or copy generated caches.

## Pending FileIO/XML compiler checkpoint — 2026-09-16

- [x] Apply the frozen 17-file FileIO/XML proposal to the existing combined
  review stage after verifying every baseline and result hash.
- [x] Compile native and WASM test targets with the shared target and
  `CARGO_INCREMENTAL=0`. Native reported 20 diagnostics; WASM reported 22,
  totaling 11 distinct issues after duplicate lib/test diagnostics are removed.
  Evidence: combined-stage `native-tests-review-20260916T180526587144Z` and
  `wasm-tests-review-20260916T180613044260Z`. Both source manifests were unchanged.
- [x] Recompile the corrected 18-file proposal, including NetManager preparation
  and reset isolation. Native reports 3 test module-path errors; WASM reports
  those 3 plus 3 test async/block-on errors. Evidence: combined-stage
  `native-tests-review-20260916T181741430439Z` and
  `wasm-tests-review-20260916T181924781404Z`; both source snapshots unchanged.
  No runtime tests executed for this still-failing checkpoint.
- [x] Native test-target compile is clean at
  `native-tests-review-20260916T182521061639Z`; its exact library binary passes
  436 tests, 0 failed, 0 ignored (0.34 seconds, default stack).
- [x] With the subsequent test-only WASM gating and native result assertion,
  all WASM test targets compile cleanly at
  `wasm-tests-review-20260916T182745911858Z`, with unchanged source hashes.
- [ ] Verify the final test-only delta natively and run the real browser FileIO
  fixture alongside the existing owner, socket, and nested-input regressions.
  Browser gate `browser-isolation-runtime-20260916T182827Z` is running in the
  combined review stage. The 18-file proposal remains unaccepted into live source.
- [ ] Correct the complete compiler batch and preserve invocation-time network
  configuration and legacy FileIO failure codes; then rerun compiler gates and
  runtime tests. This proposal is not accepted into live source.
- [ ] Execute prepared FileIO network work outside the VM borrow without the
  legacy task launcher. Preserve synchronous native file preload behavior and
  wake pending completion waiters.
- [ ] Detach pending network task state on owner reset so an old completion
  cannot supply data to a fresh-generation open of the same path.

## Active follow-up: session reset and SysMenu

- [ ] Wire `NetManager::reset_owner` into `RuntimeSession::reset_player_owned`
  and verify late completion through that real session path. The earlier
  helper-level passing test did not prove this integration; the prior broad
  reset-isolation claim is withdrawn until this regression passes.
- [x] Prepare a frozen five-file SysMenu compiler checkpoint with verified
  baseline/result hashes and a source rollback archive in the combined stage.
- [ ] Compile native/WASM targets; add exact manager/player ownership checks
  and real browser host-effect coverage before accepting SysMenu.

## Pending playback and host-sink review — 2026-09-16

These findings concern source-only pilot stages, not accepted live changes.

- [ ] Fence playback finalization by owner and loop epoch, including the
  `is_playing` mutation; verify stop/replay cannot be stopped by the old loop.
- [ ] Preserve initialization and stop lifecycle semantics across first play,
  repeated play, replay, movie replacement, and explicit restart.
- [ ] Remove overlapping mutable SoundManager/player references and use
  owner-qualified Flash loading status in the owned playback executor.
- [ ] Settle host-sink waiters on reset, detach, and disposal, including callback
  exceptions; prevent old-generation events from settling fresh waiters.
- [ ] Surface mailbox overflow from production producers without losing ordered
  transitions. Native overflow handling must use a portable error rather than
  constructing `JsValue` on a native target, which can panic.

## Six-stage checklist

- [x] **Stage 1 — Baseline and ownership inventory.** The scoped browser
  baseline passes `dpt_shapes` 1/1 and the focused Director audio regression
  passes all 8 cases. The complete licensed movie campaign remains unrun.
- [ ] **Stage 2 — Shared-runtime ownership refactor.** The allocator ownership
  subincrement is accepted with 10 native tests, 10 Miri tests, the focused
  browser regression, and the regular `wasm-pack` build passing. This is a
  subincrement, not completion of the full stage.
- [ ] **Stage 3 — Independent sessions.** Prove interleaved and separate-thread
  sessions, overlapping local IDs, reset cancellation, late completions, and
  cleanup.
- [ ] **Stage 4 — Native Director slice.** Add the native resource, execution,
  bitmap, audio, input, reset, and deterministic capture interfaces.
- [ ] **Stage 5 — Ruffle and Bevy integration.** Prove Spybot title lifecycle,
  Flash frame 419, START interaction, and tower-3 entry with a concrete input
  sequence.
- [ ] **Stage 6 — Parity and certification.** Add native backend selection,
  certify repeatability, performance, and fidelity, and retain browser
  comparison.

## Accepted Stage 2 allocator subincrement

The captured allocator ownership snapshot reports passing native and Miri tests:

- Native: 10 passed, 0 failed, in
  `.cache/native-bevy/evidence/native-bevy-allocator-ownership-final-20260914T052400Z.log`.
- Miri: 10 passed, 0 failed, under `nightly-2026-06-16`, in
  `.cache/native-bevy/evidence/native-bevy-miri-allocator-ownership-20260914T051432Z.log`.
- Browser: 1 focused `dpt_shapes` test passed, in
  `.cache/native-bevy/evidence/browser-shapes-current-last-run.json`.
- Regular WebAssembly: `mise run build:vm` passed and produced `vm-rust/pkg`,
  in `.cache/native-bevy/evidence/native-bevy-build-vm-20260914T051835Z.log`.
- Captured audio: all 8 focused cases passed against the dirty working-tree
  artifact labeled `297da4a+working-tree`, in
  `.cache/native-bevy/evidence/audio-current-working-tree.json`.

The associated captured working-tree snapshot has `git HEAD=297da4a` and a
timestamped `source-manifest-current-*.txt` file under
`.cache/native-bevy/evidence/`. The latest manifest covers every modified
tracked and relevant untracked source, documentation, tooling, and manifest
file, including `ownership.rs`; it excludes cache, build, target, generated
public, and test-result outputs. This is a dirty-tree capture from that run,
not a frozen baseline hash.
The pre-refactor browser runner is preserved in
`.cache/native-bevy/baseline-297da4a/`.

The historical allocator snapshot leaves material ownership work unresolved:
it has no `RuntimeSession` proof, and global symbols and legacy active-player
accessors remain in that captured boundary. Bitmap ownership also remains
unresolved: raw numeric bitmap IDs can collide across managers, and the public
`get_datum_mut` API permits arbitrary datum-variant replacement. Existence
checks or a replacement helper alone cannot close those gaps; owner-qualified
bitmap handles or a checked transfer boundary plus typed/guarded datum mutation
are still required. The ownership scanner gate therefore remains a future
refactor check, rather than evidence that Stage 2 is complete. No native player
or parity worker has been implemented yet. The current full VM integration
remains unbuildable, as shown by the library-check failure below.

## Active Stage 2 symbol and session migration

As of 2026-09-14, the runtime pilot is actively migrating symbol and player
ownership toward an explicit session/player context. The current scope removes
ambient `Symbol` APIs, introduces the session context, and updates cast and
interpreter consumers to receive that context. The primary changes are in
`vm-rust/src/player/symbols/{symbol.rs,symbol_table.rs}`, the new
`vm-rust/src/player/session.rs`, and dependent player/director call sites. This
work is implemented in the uncommitted working tree but is not yet verified.

The navigator checkpoint library check was run with:

```sh
mise exec -- cargo check --manifest-path vm-rust/Cargo.toml --lib --locked
```

The earlier full-VM checkpoint failed with 1,521 errors; its retained output is
`/private/tmp/native-bevy-cancel-consume-check.log`. The current dirty checkout
therefore remains unbuildable. This symbol/session migration remains unchecked
in the six-stage checklist. The allocator, browser, audio, and regular
WebAssembly results above remain historical evidence for their captured
artifacts and do not promote the current migration. The current source files
have changed during formatter recovery, so these results carry no current
source-unchanged claim.

The builtin-literal source checkpoint migrated 71 literal constructors to
canonical variants from the existing exact 975-entry builtin-ID fixture. Forty
one dynamic literals were intentionally left for a later context-aware change.
The uncommitted source remains unverified: the native library check failed with
1,449 errors and 270 warnings, retained in
`/private/tmp/native-bevy-builtin-literal-check-final2.log`, and the WebAssembly
library check failed with 1,453 errors and 267 warnings, retained in
`/private/tmp/native-bevy-builtin-literal-check-wasm-final2.log`. Neither
checkpoint reported stack or JS-bridge signature diagnostics, but the full VM
remains unbuildable and no tests were executed. These logs are preserved as
historical checkpoints and do not promote the migration or Stage 2.

The owned handler-code/name increment is implemented in the uncommitted source:
`HandlerCode` retains `Rc<Script>`, `Rc<HandlerDef>`, and cast-owned
`Rc<[Symbol]>`, so the former raw code/name pointers are removed. The CastManager
const mismatches and cast-notification `u32` mismatches found during that
increment were corrected. The source remains unverified: the native library
check failed with 1,440 errors and 270 warnings, retained in
`/private/tmp/native-bevy-owned-handler-check-final3.log`, and the WebAssembly
library check failed with 1,444 errors and 267 warnings, retained in
`/private/tmp/native-bevy-owned-handler-check-wasm-final.log`. Test compilation
failed with 1,447 errors and 274 warnings, retained in
`/private/tmp/native-bevy-owned-handler-testcompile-final4.log`; tests remain
unexecuted. These checkpoints do not promote the handler-code increment or
Stage 2.

The ScopeToken source checkpoint is implemented in the uncommitted source under
review. Tokens validate owner, invalidation epoch, slot generation, and (for
teardown or result delivery) top-of-stack position. Setup captures the owned
HandlerPlan before virtual or JavaScript callbacks; eager movie continuation
preserves the retained old frame, and epoch exhaustion is checked. Setup also
revalidates after the trace callback before clearing tracker state. These are
source-review findings only. The native test-compilation checkpoint ran to
completion with return code 101, 1,447 errors, and 274 warnings, in
`/private/tmp/native-bevy-scope-token-testcompile-final6.log`; it added no new
test-module diagnostics. Tests were not executed and no acceptance claim is
made. The native and WebAssembly ScopeToken library checkpoints
(`/private/tmp/native-bevy-scope-token-check-final.log` and
`/private/tmp/wasm-bevy-scope-token-check-final.log`) are historical only, not
proof of a verified full migration.

The compiled-IR source checkpoint is implemented and source-reviewed. The
player-aware runner uses explicit player/token arguments and safe scope
indexing; it has no raw scope pointer or ambient runner helper. The review
found no identified runtime blocker in the preserved allocation order,
comparison borrowing, copy-on-write handling, or control-flow behavior. Its
focused tests use real owned `DirPlayer` instances and cover integer loops,
escape/resume, stale or foreign tokens, typed arithmetic, and string copying.
The native library check still failed with 1,440 errors and 269 warnings
(return code 101), retained in
`/private/tmp/native-bevy-safe-ir-check-final3.log`; test compilation failed
with 1,447 errors and 273 warnings (return code 101), retained in
`/private/tmp/native-bevy-safe-ir-testcompile-final5.log`. No new IR-specific
diagnostics were identified, and tests remain unexecuted. This is a
source-review checkpoint only: it does not establish native runtime behavior,
browser parity, or Stage 2 acceptance. The prior 1:1 IR test assertion and
benchmark comment are being restored after this diagnostic checkpoint.

The next async boundary remains unresolved: bytecode leaf handlers still write
scope or `last_handler_result` after awaits (`ExtCall`, `ObjCall`,
`ObjCallV4`, and `NewObj`), global-handler return and `passed` propagation still
use the ambient helper, and the datum-handler depth/description wrapper restores
state through ambient player access after awaits. The trace path also dispatches
through a borrowed host player while allowing re-entry; post-callback token
validation cannot make that borrow safe. These gaps keep the full interpreter
migration unverified.

The active cast/session work has added private immutable metadata to cast
requests, matching-capability cancellation/restart handling, and a
duplicate-completion regression. Source review also covers the ordered-cast
edge cases for a cache snapshot behind a network request, per-player ordering,
stale-owner rejection, cast replacement, and one-time readiness publication.
The corresponding tests are present but have not been executed. The latest
test-compilation attempt was:

```sh
mise exec -- cargo test --manifest-path vm-rust/Cargo.toml --lib --no-run --locked
```

It failed with 1,529 errors and 275 warnings; the retained output is
`/private/tmp/native-bevy-ordered-preload-testcompile.log`. This is distinct
from the earlier 1,521-error library-check checkpoint above. These changes are
not accepted behavior evidence because full-VM test compilation is still
blocked.

The ordered-cast batch must preserve the original cast sequence, including
cache hits, with a matching-capability barrier and retaining in-flight mode-2
loads across repeated preload calls. The source-reviewed tests are
`cached_snapshot_waits_behind_network_cast`,
`interleaved_players_drain_only_their_own_cast_order`,
`stale_owner_queue_entry_is_removed_without_touching_current_barrier`,
`replaced_cast_reserves_fresh_capability_before_preload_check`, and
`duplicate_buffered_completion_is_rejected`. Remaining dependencies are
deferring `castapply` effects from its global cache/font/palette state and
JS-Lingo initializer re-entry into the explicit owner context, adding a live
browser request adapter, and qualifying the outbox with its owner.

The explicit stack-materialization increment is complete in the source under
review. `OperandStack` now stores
`StackDatum` directly, with explicit allocator/bitmap parameters for
materializing consumers and no remaining production `into_ref()` call. The
three focused allocator-backed tests cover cached materialization, owner-arena
isolation, and dropping inline values without materialization. They do not
cover full interpreter execution, marker/deque extraction, or the remaining
context-consumer migration, and none were executed because the full VM does
not compile.

The final library-check checkpoint was run with:

```sh
mise exec -- cargo check --manifest-path vm-rust/Cargo.toml --lib --locked
```

It failed with 1,518 errors and 269 warnings; the retained output is
`/private/tmp/native-bevy-stack-caller-check-final.log`. The final test
compilation checkpoint was run with:

```sh
mise exec -- cargo test --manifest-path vm-rust/Cargo.toml --lib --no-run --locked
```

It failed with 1,526 errors and 273 warnings; the retained output is
`/private/tmp/native-bevy-stack-caller-testcompile-final2.log`. These are
source-review checkpoints only; no stack tests or runtime behavior are
accepted from them. The interpreter/context-consumer migration remains pending.
The next proposed ownership seam is the canonical owned `DriverContinuation`
context migration, which must replace the ambient trampoline path and is not
yet implemented. It will subsume the async result-only boundary for bytecode
leaf handlers, global-handler return propagation, and datum-handler bookkeeping.
The ScopeToken and compiled-IR source checkpoints above do not make that
migration verified.

## Isolated symbol-core harness evidence

The isolated harness in `tools/symbol-ownership-tests` includes the production
symbol files directly and has no `vm-rust` dependency. Its tests validate the
symbol core and table ownership behavior; they do not validate `RuntimeSession`,
the interpreter/cast integration, or full VM compilation.

- Native harness: 15 passed, 0 failed, in
  `.cache/native-bevy/evidence/native-bevy-symbol-ownership-native-final-20260914T061649Z.log`.
- After formatter recovery, `mise run test:symbol-ownership` was rerun and
  passed all 15 tests; the navigator retained command output without a durable
  log path, so no additional artifact link is claimed here.
- Historical Miri harness: 12 passed, 0 failed, under
  `nightly-2026-06-16`, in
  `.cache/native-bevy/evidence/native-bevy-symbol-ownership-miri-20260914T060945Z.log`.
- Lifecycle-only Miri: 3 passed, 0 failed, with 12 filtered out, under
  `nightly-2026-06-16`, in
  `.cache/native-bevy/evidence/native-bevy-symbol-ownership-miri-lifecycle-20260914T061819Z.log`.

These are captured-artifact results. Production symbol files were changed by
formatter recovery afterward; the pre-formatter copies and hashes remain in
`.cache/native-bevy/checkpoints/symbol-session-wip-20260914T054954Z/`.

## Baseline scope and retention

The legacy `/private/tmp/*check*.log` and `*testcompile*.log` paths recorded
above are historical references. All 14 referenced temporary logs were absent
when checked during this recovery. Their earlier reported counts cannot be
independently rechecked from those paths; use the newer checkpoint evidence
under `.cache/native-bevy/evidence/` for current compilation diagnostics.

Stage 1 covers the available synthetic shapes fixture and the recovered Spybot
audio fixture. It does not establish full licensed movie parity. The original
failed title-ready audio JSON was removed by a later Playwright run that clears
`test-results`; its retained log was not available for durable reconstruction.
The passing frozen audio JSON above is explicitly labeled as reconstructed from
its retained log, and the current dirty result is retained separately.

The evidence directory is ignored local output. It contains the runner hashes,
source hash manifest, browser/build/Miri logs, and both frozen/current audio
records needed to reproduce or review this snapshot.

## Active recovery sequence

The resumed work prioritizes a compiling, behavior-preserving Stage 2 runtime
before native integration. The interrupted driver draft is incomplete; it is
not execution evidence. Its predecessor is retained in
`.cache/native-bevy/checkpoints/driver-context-start-20260914-102811/`.

- [ ] Recover and review driver completion validation, cancellation, and exact
  execution phases against the retained predecessor.
- [ ] Complete explicit context and symbol propagation through production
  callers, preserving opcode and browser extension behavior.
- [ ] Restore full native and WebAssembly compilation and execute the pending
  ownership, scope, compiled interpreter, and continuation tests.
- [ ] Complete session-owned scheduling and callback boundaries; prove reset
  and stop remain responsive while work is pending.
- [ ] Re-run browser regressions and ownership gates before Stage 3 acceptance.

The existing six-stage checklist remains the full scope. Historical passing
artifacts above do not validate this recovery tree.

### Completion-boundary recovery checkpoint

The first resumed source checkpoint adds exact completion capability, action
kind, resume-phase, scope, and result-owner validation. Accepted results remain
owned in an explicit resume state; rejected results leave valid pending work
intact. Player removal unwinds valid frames and retires actions by actual owner
identity. These are source-reviewed changes, not complete driver behavior.

The fresh native test compilation completed with return code 101, 1,450 errors,
and 280 warnings. No primary compiler diagnostics referenced `driver.rs` or
`session.rs`; no tests executed. Evidence, command result, and source hashes are
stored under `.cache/native-bevy/evidence/` with prefix
`native-bevy-completion-recovery-20260914T161405Z`.

The next bounded correction closes terminal-error frame cleanup and retires
permanently invalidated pending drivers on their next turn. Applying retained
completion payloads, faithful opcode/IR/debugger transitions, and production
root scheduling remain unfinished. The canonical owner-aware datum conversion
and formatting API is designed and remains unimplemented at this checkpoint.

The subsequent cancellation closure is source-reviewed: terminal turns and
player removal share validated frame unwinding; permanently invalid pending
frames retire on their next turn, while temporarily non-top active frames stay
pending. Teardown retains a child token until success so stale-child cleanup
cannot start at its parent. Tests for these paths are present but unexecuted;
this follow-up postdates the compilation evidence above.

The snapshot `driver-cancellation-20260914-121831` was captured after the
production cancellation edits and before their final test correction. It is
not a snapshot of the source before the cancellation increment.

### Owner-aware datum API checkpoint

Datum conversion and recursive formatting now require the supplied owning
symbol table. Foreign dynamic symbols return errors, builtin symbols remain
portable, numeric formatting retains its generic fallback, and invalid datum
references are checked before depth abbreviation. Local tests cover those
boundaries with production tables and players. Source review accepted this API
checkpoint and the preceding cancellation closure; integrated execution is
still unverified.

The combined native test compilation completed with return code 101, 1,980
errors, and 277 warnings. No primary errors referenced `driver.rs`,
`session.rs`, `datum_formatting.rs`, or `director/lingo/datum.rs`; no tests
executed. The increase includes downstream callers that now require explicit
table arguments and handling of fallible formatting. Evidence, command result,
and source hashes use prefix
`.cache/native-bevy/evidence/native-bevy-datum-context-20260914T163415Z`.

The next source increment propagates the table through comparison and
arithmetic helpers, followed by compiled execution and synchronous bytecode
consumers. Root scheduling and the remaining driver execution phases remain
unfinished.

### Comparison and arithmetic API checkpoint

- [x] Source-review explicit symbol tables through comparisons, arithmetic,
  concatenation, and sorting.
- [x] Reject direct foreign symbols and symbolic 3D names at contextual
  comparison boundaries, including zero checks.
- [x] Add cycle-aware sorting preflight for reachable lists/property lists,
  with singleton foreign-symbol rejection and distinct-string stable-order tests.
- [ ] Execute these tests once the integrated VM compiles.
- [x] Connect compiled arithmetic/comparison and their synchronous bytecode consumers.

The preflight adds work and storage proportional to the reachable datum graph.
Ordinary comparisons retain visited-branch behavior, including the existing
property-list identity shortcut. Existing comparator error and cyclic-comparison
behavior is preserved; only the preflight is claimed to terminate on cycles.

The pilot's native no-run compilation reports 2,004 errors and 280 warnings,
with no errors in either changed file. No tests executed. The preserved log uses
prefix `.cache/native-bevy/evidence/native-bevy-compare-context-20260914T165107Z`.
The adjacent handoff hashes were captured after compilation, not before it.
The pre-edit snapshot is `compare-datum-ops-20260914-124106`.

### Compiled and arithmetic/comparison bytecode checkpoint

- [x] Source-review disjoint player/table borrowing and explicit table propagation
  through compiled arithmetic, comparison, zero checks, and resumable runners.
- [x] Preserve scope validation before mutation, integer fast paths, and existing
  operand materialization order.
- [x] Validate both inline symbols before bytecode equality/inequality shortcuts;
  add real-context local/foreign tests and an allocation-free equality assertion.
- [ ] Execute those regression tests after full compilation is restored.
- [x] Complete string bytecode and context-variable table propagation.

Native no-run compilation returned 101 with 1,977 errors and 280 warnings.
There are no errors in `session.rs`, `compiled/mod.rs`, or the arithmetic and
comparison bytecode files. `player/mod.rs` still has errors outside the two
synthetic benchmark callsites changed here. No tests executed.

The pre-edit snapshot is `execution-context-compiled-bytecode-20260914-170200`.
Command, output, result, and touched-source hashes are under
`.cache/native-bevy/evidence/native-bevy-execution-context-compiled-bytecode-20260914T171300Z/`.
The subsequent test-only correction uses the existing `datum_count()` API and
adds a local inequality case. Its separate hash is recorded there; parse and
whitespace checks passed, and it has not had another compile attempt yet.

### String and context-variable production checkpoint

- [x] Source-review table propagation through string bytecode and context get/set.
- [x] Preserve custom versus canonical Null concatenation and conversion order.
- [x] Validate field symbols before ordinary field-error compatibility handling.
- [x] Add direct JoinStr and PutChunk regression fixtures using production contexts.
- [ ] Execute the regression fixtures after compilation is restored.

Native no-run compilation returned 101 with 1,954 errors and 279 warnings;
there are no diagnostics in the two changed files. No tests executed.
Evidence and touched-source hashes are in
`.cache/native-bevy/evidence/native-bevy-string-context-vars-20260914T173500Z/`;
pre-edit sources are in `string-context-vars-20260914-172000`.
These directory labels identify checkpoints, not independently verified precise
execution timestamps. Direct opcode test additions postdate this compile.

### Driver PC and ordinary completion checkpoint

- [x] Source-review normal completion for empty/exhausted handlers and final Advance.
- [x] Preserve final PC, increment nonfinal Advance once, then honor stop requests.
- [x] Validate the parent and advance its PC once before child setup.
- [x] Add real setup-success, setup-error, virtual Early, and stale-parent fixtures.
- [ ] Execute driver fixtures after integrated compilation is restored.
- [x] Connect LocalCall bytecode synchronously to the child-frame path.
- [ ] Implement retained completion application and remaining IR/debugger phases.

Native no-run compilation returned 101 with 1,954 errors and 279 warnings;
there were no driver diagnostics. This compile includes the expanded string
opcode fixtures. No tests executed. Evidence is in
`.cache/native-bevy/evidence/native-bevy-driver-pc-20260914T173239Z/`.
The compiled driver hash starts `45aacd87`; import cleanup and explicit test
unwraps followed that compile. The subsequent Early and actual setup-error
fixtures also postdate it, with parse and whitespace checks recorded in
`.cache/native-bevy/evidence/native-bevy-driver-early-error-20260914T173907Z/`.
Direct child-handoff tests do not yet establish LocalCall opcode integration.

### LocalCall context checkpoint

- [x] Source-review synchronous LocalCall dispatch and removal of its placeholder action.
- [x] Preserve argument marker/materialization order, receiver precedence, raw arguments,
  caller fallback, and return-push behavior; the driver owns the single PC increment.
- [x] Reject foreign handler symbols and invalid datum/instance refs during resolution.
- [x] Add real driver LocalCall/child-return and receiver/ancestor/sprite-order fixtures.
- [ ] Execute these fixtures after full compilation is restored.
- [x] Source-review property-list handlers and their sub-property/duplication dependencies.

Native no-run compilation returned 101 with 1,949 errors and 277 warnings.
There are no errors in the driver or newly changed LocalCall/helper test regions;
other regions of the touched files still have migration errors. No tests executed.
The unsafe legacy global-call caller has no session table and remains an explicit
migration dependency. Missing ScriptRef targets and invalid sprites now produce
ScriptError instead of indexing/lookup panics; missing handlers retain fallback.
Evidence and source hashes are in
`.cache/native-bevy/evidence/native-bevy-local-call-20260914T175824Z/`.

### Protocol compatibility fixtures

- [x] Add standalone synthetic request, response, error, and compatibility fixtures.
- [x] Independently verify 20 request round trips, 19 response decodes, and all 22
  compatibility classifications against the actual reference Rust parity crate.
- [x] Validate 61 records and POSIX transport checks, including bounded writes/reads,
  line limits, UTF-8, malformed values, stderr drainage, and child-group cleanup.
- [x] Add `mise run test:protocol-fixtures`; the integrated task passes.
- [ ] Implement the production native worker and verify its real responses.

The cleanup regression uses an exited parent and a descendant that ignores
SIGTERM; bounded pipe EOF verifies termination after escalation. These are
transport fixtures, not VM execution, native capture, or fidelity evidence.
The validator has no dependency on childhood-redux. Development-only reference
harness source, lockfile, output, and checkpoint hashes are retained in
`.cache/native-bevy/evidence/protocol-fixtures-review-20260914T175152Z/`.
The final cleanup correction was independently rerun successfully; subsequent
mise integration changed only task configuration and fixture documentation.

### Interrupted property-list recovery

- [x] Finish and review the five-file property-list context migration.
- [x] Preserve instance-property string interning, chained built-in fallback,
  authoritative handler-name casing, and the existing getter return path.
- [x] Validate mutation operands before changes; keep comparison preflight
  limited to the operations and candidates that require it.
- [x] Add production-context regression fixtures and capture compile diagnostics.
- [ ] Execute fixtures after integrated compilation is restored.
- [x] Migrate and source-review linear-list handlers through the explicit context API.

The corrected production source has passed review. The original comparison oracle is
`.cache/native-bevy/checkpoints/prop-list-context-preedit-20260914T180138Z/`.
After the interruption, no prior pilots or compiler processes remained active.
A replacement Luna High pilot completed the same five-file recovery and released
writer ownership. The linear-list handler migration is now the sole active
shared-runtime write batch; constructor follow-ups remain read-only design work.

The final property-list compile checkpoint is
`.cache/native-bevy/evidence/native-bevy-prop-list-recovery-final-20260914T191908Z/`.
The exact native no-run command returned 101 with 1,951 errors and 281 warnings;
there were no errors in `prop_list.rs`, including its new fixtures. No tests
executed. Precompile and postcompile source hashes match. The final small
diagnostic correction and its fixture postdate this compile: valid Void now
formats as Void, while foreign references produce explicit diagnostic text.
They will be included in the next linear-list compile checkpoint.

Fixtures use real players, symbol tables, and ExecutionContext dispatch. Built-in
property tests exercise the helper, not a chained opcode; the chained PropList
branch was source-reviewed for equivalent fallback and allocation behavior.
Constructor/JS setup and virtual-script migrations remain design work.

Post-review diagnostic correction evidence is retained in
`.cache/native-bevy/evidence/native-bevy-prop-list-recovery-postreview-20260914T192328Z/`.
The accepted property-list source hash is
`652fd29e83da34c81ab5b366649567a2b13fdedeec2a785f99c734264a49558e`.
Parse checks and `git diff --check` were reviewed; the four existing rustfmt
trailing-whitespace diagnostics in `get_set.rs` remain outside this increment.


### Linear-list context recovery

- [x] Remove ambient player access from linear-list dispatch and helpers.
- [x] Review indexing, mutation notices, stable sorting, Void handling, join
  formatting, duplication, and the W3D texture-layer creation branch.
- [x] Add real session fixtures for indexing, unknown foreign property names,
  visited foreign search candidates, mutation rejection, matrix rows, sorting,
  generic/W3D add, join, and duplicate dispatch.
- [x] Capture final compile diagnostics with matching source hashes.
- [ ] Execute runtime fixtures after full compilation is restored.
- [x] Replace unchecked visited child-reference access in fallible comparisons.
- [x] Migrate and source-review virtual-script name/property dispatch through the owning table.

The retained checkpoint is
`.cache/native-bevy/evidence/20260914T200114Z-list-handlers/`.
Native no-run compilation returned 101 with 1,946 errors and 280 warnings, with
no errors attributed to `list_handlers.rs`. Tests did not execute. Precompile,
postcompile, and independently checked current list source hashes match
`8d633d7b5fcf3b4237be821b644159299af42e4e2ecf1ef2c6f44b10f050c439`.
This compile includes the accepted property-list diagnostic correction at hash
`652fd29e83da34c81ab5b366649567a2b13fdedeec2a785f99c734264a49558e`.
The earlier list handoff count lacked located supporting evidence and is not
used for acceptance. Source review caught and corrected a sort container type
error, Void sentinel handling, and unchecked matrix/join element access.

The W3D fixture exercises one actual mesh context; multi-context selection is
source-reviewed only. Cache fixtures check script-list generation; they do not
prove every actor-cache path. Recursive comparison still has unchecked child
lookups in its fallible functions. That dependency is the next bounded write
batch, preserving operand order and short-circuit behavior without adding whole
graph scans to ordinary comparisons.


### Checked recursive comparison references

The compare-only follow-up replaces 17 child-reference loads in fallible
comparison paths with checked lookup, preserving the Void sentinel, operand
materialization order, short-circuit behavior, and ordinary comparison results.
The existing sort preflight and unwrap comparator remain unchanged. Two fixture
groups cover List equality/ordering, PropList keys/values, Void, and early
mismatches that skip later foreign references. PropList greater-than retains
its existing unsupported-type false result without inspecting child values.

Compile evidence:
`.cache/native-bevy/evidence/native-bevy-compare-compile-20260914T200838Z/`.
The no-run compile returned 101 with 1,946 errors and 280 warnings and no errors
in `player/compare.rs`; tests remain unexecuted. Removing an incorrect fixture
assertion after that compile produced accepted hash
`4a5f9051f8b02f7a1562dfcda0ee4eb64ae83f22341e5fc88af0357c2b56fa19`.
The navigator checked the final diff/hash and parse-only rustfmt successfully.
A formatting-style check failed on existing drift; no broad formatter was run.
The next virtual-script compile will include the final fixture correction.
Virtual-script dispatch is now the sole active shared-runtime write batch.


### Virtual-script dispatch ownership

- [x] Thread the owning symbol table through virtual handler/property APIs and
  JavaScriptProxy; reject foreign names before absent/default fallbacks.
- [x] Validate instance owners against their allocator and use checked live
  lookup where needed; preserve the empty property-registry fast path.
- [x] Preserve registration, map order, canonical proxy lookup, inherited
  receivers, direct calls without new handler gates, and allocation order.
- [x] Pass the session table at the existing handler setup boundary.
- [x] Review real registry fixtures and retain compile evidence.
- [ ] Execute virtual registry/driver fixtures after full compilation returns.

Evidence is in
`.cache/native-bevy/evidence/checkpoints/constructor-design/20260914T202603Z/`.
The locked no-run compile returned 101 with 1,954 errors and 280 warnings. No
errors were attributed to the virtual-script files or changed setup closure;
existing warnings remain. Unmigrated callers include fallout from the changed
signatures. No tests executed. Parse-only checks and diff checks passed.

The accepted hashes are virtual registry
`3e5f6a6ba34fddf19913c8739c0ad69d45d3979ac274be54655f97784e68ed72`,
JavaScriptProxy
`6691f030c36f8fa70538452c69454f3761a18625994fda181656d6d72d4aaf3c`,
and player module
`0d9ce69fa0c45d4c3663b1c0d14b1e55c159fce0b8e8579bed1dd9717e131fb2`.
Stored baseline references were materialized after edits, as documented in the
manifest. The reconstructed player-module baseline matches the independently
recorded prior accepted hash; its only incremental diff is the setup closure.

CastMemberRef remains numeric and cannot distinguish all foreign member refs.
This increment does not implement queued JS/trace host callbacks. Synchronous
sprite comparison and chunk-variable bytecode are now the active write batch;
property-read consumers are queued next.


### Synchronous sprite/chunk bytecode

- [x] Pass session symbols through `push_chunk_var_ref`, `onto_sprite`, and
  `into_sprite`, with top-scope validation before bytecode/stack access.
- [x] Check accessed sprite operand references and foreign symbols before
  conversion, retaining Void/local-symbol compatibility and borrowed values.
- [x] Keep diagnostic formatting infallible and independent of log enablement.
- [x] Review real overlap/containment, foreign operand, local symbol, stale
  scope, field identifier, and malformed-formatting fixtures.
- [ ] Execute fixtures after full VM compilation is restored.

Evidence: `.cache/native-bevy/evidence/20260914T203737Z-bytecode-context-final/`.
The stable-source no-run compile returned 101 with 1,939 errors and 278 warnings.
There were no errors in sprite comparison or new fixture regions; the remaining
stack error is the deferred async NewObj display call. Pre/post/current hashes
match: stack `3b2cd0df2a233708f463dc3c13a777fe40c78e7d8038e61fb49966b796a8d649`
and sprite comparison
`8f8266228205b5d6439e4709051cd3dcc11e0531a53b3ed8da9867f867d51e02`.
Parse-only checks and diff checks passed. No tests executed. The earlier
203456 checkpoint changed source hashes during compilation and is not used as
verification of the corrected files.

Sprite fixtures call the production leaf handlers directly; the field fixture
uses the static bytecode dispatcher. Other leaf/dispatcher scope-token guards,
async NewObj, and ambient property delegates remain separate work. Property-read
getters and their explicit-context callers are now the sole runtime write batch.

## Active compile recovery priority

The resumed implementation prioritizes restoring the shared-runtime build before adding native features. Existing accepted source increments and historical results above keep their stated verification limits.

- [x] Capture a fresh frozen-source native test compilation and group the remaining caller migrations.
- [x] Finish and review the interrupted property-read source increment, including callback invalidation checks and ordinary missing-property behavior; regression fixtures remain unexecuted pending a compiling VM.
- [ ] Migrate remaining symbol/context callers in coherent batches, checking each batch against the compiler.
- [ ] Obtain a passing native library/test compilation and execute the shared-runtime tests.
- [ ] Restore the WebAssembly build and focused browser regressions before proceeding with native integration.

Compile-recovery baseline at `20260914T210338Z`: `mise exec -- cargo test --manifest-path vm-rust/Cargo.toml --lib --no-run --locked --message-format=json` exited 101 with 1,931 errors and 278 warnings. The retained `.cache/native-bevy/evidence/compile-recovery-20260914T210338Z/` contains the command, diagnostics, exit code, category/file breakdown, and identical before/after Rust-source and Cargo-manifest hashes. No tests executed. The largest categories are 922 E0599 removed-method errors and 549 E0061 argument-count errors; the largest single file is `shockwave3d_object.rs` with 347 errors.

Property-read recovery source review: `script.rs` now restores the checked warning snapshot and revalidates the original receiver after ancestor callbacks. The three read opcodes validate callback results before propagating errors or changing scopes/caches; chained reads preserve the Void sentinel and check visited getter results. Two native fixtures cover replacement-scope preservation for callback success/error and chained Void access. The frozen `20260914T211530Z` test compilation exited 101 with 1,928 errors and 278 warnings, with no errors in the migrated read regions or new fixtures; no tests executed. Evidence and identical source manifests are under `.cache/native-bevy/evidence/compile-recovery-20260914T211530Z/`; the recovery pre-edit snapshot is retained under `.cache/native-bevy/evidence/property-read-recovery-preedit-20260914T210338Z/`. Existing setters and ambient delegates remain unmigrated. Rustfmt emit checks reported existing WIP whitespace failures; the compiler parsed the corrected files.

The next active compiler-recovery batch owns `shockwave3d_object.rs`: explicit player/table parameters, table-backed name conversion, and non-Copy symbol propagation through its helper callgraph. The parser/decoder and their cast-loading callers are queued separately.

Parser recovery source applied after review: `w3d/{mod,parser,clod_decoder}.rs` now use the supplied mutable symbol table for parsing and generated mesh names. The existing cast-loading table is threaded through `CastMember::from` and both `scan_children_for_ole` callers. Normal interning preserves first spelling; root-COM map keys retain the original Symbol interner normalization. A frozen isolated overlay against the `211530Z` baseline compiled with 1,878 errors and 278 warnings (50 fewer), with zero errors in parser/decoder/module code; pre-existing cast-lib/cast-member errors remain. No tests executed. Both moved-name errors found in the first isolated build were corrected before the verified second build. The five live files matched the recorded before hashes and verified isolated after contents before application. Evidence is retained at `.cache/native-bevy/evidence/parser-recovery-20260914T213700Z/`. This is isolated-overlay evidence, not a combined-checkout build; W3D handler work continues.

Combined recovery checkpoint `20260914T214019Z`: the reviewed W3D getter caller integration and decoder table argument are applied. `get_obj_prop` now accepts the mutable session table; only its W3D branch owns display text, and the chained bytecode path owns its property text before mutable getter calls. The frozen live test compilation (`--offline`, isolated target directory) exited 101 with 1,741 errors and 278 warnings, down from the initial continuation baseline of 1,931. Source hashes match before/after. Evidence is retained at `.cache/native-bevy/evidence/compile-recovery-20260914T214019Z/`. No tests executed. The W3D object handler is partial and unaccepted, with 206 remaining errors (previously 347); old `as_str_in` uses still need fallible resolution and other conversion/caller work remains. A fresh Luna High pilot is assigned only its top-level getter region to keep recovery increments bounded.

Service interruptions during this continuation included explicit Luna model-capacity failures and a remote-compaction capacity failure. An elevated compile was also rejected because automatic approval review itself hit model capacity; an offline sandbox build with a temporary target directory succeeded as a safer alternative, and later hash-checked source applications were approved normally. No model switch or publication occurred.

Parser normalization review correction: inspection of `HEAD:vm-rust/src/player/symbols/symbol.rs` showed that the legacy `Symbol::to_ascii_lowercase` method actually returns the interner lowercase spelling. The parser now resolves this key with `SymbolTable::lower`, preserving that behavior. The isolated normalized overlay remains at 1,878 errors/278 warnings with no parser errors and matching before/after hashes; evidence is at `.cache/native-bevy/evidence/parser-symbol-normalization-20260914T214552Z/`. This supersedes the navigator's earlier incorrect ASCII-only interpretation.

Scalar and W3D getter recovery checkpoint `20260914T215529Z`:

- [x] Review and integrate explicit-table Int/Float/Symbol/Void getters and four callers, preserving Void conversions and direct-symbol validation.
- [x] Review the W3D top-level getter conversion with fallible display/lower resolution; preserve collision lower-left/display-right comparison and authoritative interned bone-model spelling.
- [x] Compile frozen combined source: exit 101, 1,697 errors and 278 warnings, matching before/after hashes; no errors in migrated getter regions. This is 44 fewer errors than the preceding live checkpoint.
- [ ] Complete W3D setters, calls, and private helpers; 170 errors remain in that module.
- [ ] Execute runtime tests once compilation succeeds. No tests executed at this checkpoint.

Evidence: `.cache/native-bevy/evidence/compile-recovery-20260914T215529Z/` and `.cache/native-bevy/evidence/scalar-getter-integration-20260914T215529Z/`. Getter source acceptance does not establish safety of remaining ambient delegates or host reentry.

Geometry getter recovery: reviewed and integrated Color/Point/Rect getters with checked receiver access and explicit readonly tables, preserving receiver conversion order, palette math, inline flags, and Point.float. The unused Symbol call leaf now resolves its diagnostic with the supplied table. An isolated overlay against the frozen `215529Z` baseline compiled with 1,690 errors/278 warnings (seven fewer), unchanged source hashes, and no errors in changed getter/Symbol regions; no tests executed. Evidence: `.cache/native-bevy/evidence/geometry-getter-integration-20260914T220157Z/`. W3D setter changes were excluded using their verified pre-edit snapshot. Date/Math/Vector/Transform3d getters are the next staged batch; legacy async Void dispatch is deferred with the broader explicit session dispatcher, avoiding new ambient wrappers.

Numeric getter recovery: Date/Math/Vector/Transform3d getters and four script callers are integrated. Date validates foreign property symbols before browser Date construction and preserves receiver-agnostic ilk; vector checks consumed list references left-to-right while preserving Void/Int(0) zero vectors. The frozen isolated overlay compiled with 1,688 errors/278 warnings and no errors in changed getters; no tests executed. Four getter errors were removed, but two previously missed Transform3d::call Self::get_prop callers now need the table, for a net reduction of two. Those legacy ambient call paths remain explicit-dispatch dependencies; no new ambient wrapper was added. Evidence: `.cache/native-bevy/evidence/numeric-getter-integration-20260914T220826Z/`.

Combined getter/setter checkpoint `20260914T221607Z`: frozen native lib-test no-run compilation exited 101 with 1,659 errors/278 warnings and matching before/after hashes. No diagnostics in the migrated W3D top-level getter/setter or Timeout/SoundChannel getter regions; no tests executed. W3D module remains partial with 146 errors in calls/private helpers. Setter review corrected collision/mesh case semantics, emitter authoritative string display, accidental nested shader-symbol acceptance, fallible Option flattening, and 11 ownership moves before source acceptance. Direct symbol input validation runs before setter writes; existing nested-data and ambient-helper ownership work remains. Timeout targets and sound-channel members now reject foreign/stale visited handles while retaining Void/default behavior.

- [x] Review and compile-check W3D top-level setter source migration.
- [x] Review and compile-check Timeout/SoundChannel getters and explicit callers.
- [ ] Recover W3D private getters/calls and remove remaining ambient dispatcher dependencies.
- [ ] Obtain full compile success and execute runtime/browser tests.

Evidence: `.cache/native-bevy/evidence/compile-recovery-20260914T221607Z/` and `.cache/native-bevy/evidence/timeout-sound-getter-integration-20260914T221411Z/`. Current combined error count is 272 below this continuation baseline of 1,931; this is compiler recovery, not native feature acceptance.

Private W3D getter checkpoint `20260914T222213Z`: shader/camera/light getter source reviewed and compile-checked after correcting owned-string interning and validating selected texture names before returned object allocation. Frozen combined compile exited 101 with 1,649 errors/278 warnings, matching source hashes and no errors in those three getters (ten errors removed). W3D object module remains partial with 136 errors. No tests executed. Evidence: `.cache/native-bevy/evidence/compile-recovery-20260914T222213Z/`. Next scope is the remaining small private getters; a read-only design review is also evaluating an actual ObjCall connection to the session-owned synchronous dispatcher.

Small W3D getter checkpoint `20260914T222633Z`: node/model-resource/mesh-deform/motion/texture getter source reviewed; fallible names preserve lower-left/display-right lookup semantics, defaults, and returned names. Frozen combined native lib-test no-run compile exited 101 with 1,633 errors/278 warnings, identical source manifests, and no errors in these five functions (sixteen removed). W3D object module has 120 errors remaining. No tests executed. Evidence: `.cache/native-bevy/evidence/compile-recovery-20260914T222633Z/`. Model getter recovery is now the sole live W3D write scope. A separate staged pilot is approved for connected ObjCall-only preparation, synchronous list/proplist dispatch, and scope-checked result completion; this work is not yet integrated or verified.

Model getter checkpoint `20260914T223641Z`: get_model_prop source reviewed with fallible authoritative names, preserved first-match/short-circuit shader selection, parent/detached traversal, and queued-motion ownership. Frozen combined compile exited 101 with 1,619 errors/278 warnings, identical hashes, and no errors in the model getter (fourteen removed). W3D object module has 106 errors remaining in call/helper paths. No tests executed. Evidence: `.cache/native-bevy/evidence/compile-recovery-20260914T223641Z/`. Connected ObjCall work remains isolated and unverified.

ObjCall staged review follow-up: the first review found that successful pending-result application left the driver in Resuming, preventing later instructions from running. The pilot is correcting the phase transition and extending production-path fixtures through Ret, stale/foreign completion rejection, and dispatcher depth bookkeeping. The reported second isolated check did not reach Rust compilation because its Cargo manifest path was missing; it is not accepted evidence. A fresh explicit-manifest lib-test no-run check is required before integration. Shared source remains at the verified 1,619-error checkpoint; indexed shader-list helper recovery is the sole live write scope.

Indexed shader-list checkpoint `20260914T230258Z`: reviewed Result<bool, ScriptError> helper and its two callers, with recognized-property guard, fallible names, direct-value validation, checked cached list access, and preserved defaults. Frozen native lib-test no-run compilation exited101 with1,617 errors (two fewer), unchanged source hashes, and no errors in the helper. W3D module104 errors remain. No tests executed. Evidence: `.cache/native-bevy/evidence/compile-recovery-20260914T230258Z/`. The interrupted225315Z attempt produced no compiler result and is not verification.

Skeleton-helper checkpoint `20260914T231452Z`: frozen native lib-test no-run compilation exited 101 with 1,607 errors, unchanged source hashes (ten fewer). Reviewed skeleton lookup preserves model/resource/direct/fallback ordering with fallible table resolution; nearby non-Copy ownership fixes are applied. No tests executed. Evidence retained under `.cache/native-bevy/evidence/compile-recovery-20260914T231452Z/`.

Connected ObjCall source integrated after review: session-owned preparation, synchronous List/XmlChildNodes/PropList calls, real unsupported payload, checked completion and resume transition. Fixtures cover no-return/results, resumed Ret, depth bookkeeping, malformed same-owner and foreign-symbol results, and replacement scopes. Isolated test compilation against the preceding 1,617-error baseline reported the same 1,617 errors/277 warnings, matching before/after hashes and no diagnostics in added regions; no tests executed. A subsequent fixture-name-only correction is included and awaits combined compilation. Pending internal action execution and other opcode migrations remain incomplete. Evidence: `.cache/native-bevy/evidence/objcall-integration-20260914T231452Z/`.

Combined ObjCall checkpoint `20260914T231646Z`: frozen lib-test no-run compilation remains at 1,607 errors with unchanged hashes after the reviewed three-file integration. No tests executed. Evidence retained under `.cache/native-bevy/evidence/compile-recovery-20260914T231646Z/`. Next scopes are staged synchronous get/set bytecode caller recovery, bounded W3D transform helpers, and further synchronous datum call migration.

Relative-frame checkpoint `20260914T232056Z`: reviewed selected-argument handle validation and table-lower conversion for relativeTo, preserving argument selection and world/parent behavior, plus six clone replacements. Frozen native lib-test no-run compilation exited 101 with 1,600 errors (seven fewer), unchanged source hashes; no tests executed. Evidence: `.cache/native-bevy/evidence/compile-recovery-20260914T232056Z/`. Remaining transform helper/caller and general symbol-context migrations continue.

Void call source integrated after review: explicit ExecutionContext allocation/name resolution and synchronous dispatcher route, with an ObjCall fixture for count result and unknown-method no-return behavior. Frozen isolated overlay against the older 1,617-error baseline reported 1,616 errors/277 warnings, identical hashes, and no driver/Void errors; no tests executed. The legacy async Void caller now explicitly needs a context and remains a compiler dependency; no ambient adapter was introduced. Evidence: `.cache/native-bevy/evidence/void-call-integration/`. Combined check awaits the next W3D freeze.

World matrix/bounding sphere plus Void checkpoint `20260914T232728Z`: frozen combined lib-test no-run compilation exited 101 with 1,589 errors, unchanged hashes (eleven fewer). No errors in the reviewed matrix/bounding sphere helpers, driver, or Void handler; W3D module77 errors remain. Name lookup, descendant traversal, CLOD/raw fallback and numeric matrix/radius calculations preserve existing valid-input behavior. No tests executed. Evidence: `.cache/native-bevy/evidence/compile-recovery-20260914T232728Z/`. The legacy async Void caller remains an explicit-context dependency.

Get/set bytecode source integrated after correction/review: unused helper context parameters removed, explicit tables threaded through string conversions and diagnostics, ordered fallible global lookup with foreign-name rejection before stack writes, and direct style-value symbol validation before field mutation. One regression fixture covers foreign global-name rejection. Root reconstructed and hash-verified the accepted W3D snapshot to exclude intermediate source from the pilot overlay, then ran locked/offline frozen lib-test no-run: 1,574 errors vs accepted 1,589 baseline (15 removed), unchanged full source/Cargo hashes. Only excluded get_top_level_prop diagnostic remains in get_set.rs. No tests executed. Evidence: `.cache/native-bevy/evidence/getset-integration-20260914T233450Z/`. Combined live check follows with world-position recovery.

World-position/get-set checkpoint `20260914T233542Z`: frozen combined lib-test no-run compilation exited 101 with 1,566 errors (23 fewer than preceding live checkpoint), unchanged source hashes. Reviewed world-position and pointAt table propagation preserves first-match parent traversal, flush/init ordering and numeric early returns; legacy transform delegates remain partial. Get/set recovery is integrated. No tests executed. Evidence: `.cache/native-bevy/evidence/compile-recovery-20260914T233542Z/`.

Persistent-transform checkpoint `20260914T234218Z`: frozen lib-test no-run compilation exited 101 with 1,564 errors (two fewer), unchanged source hashes. Reviewed cached-handle validation preserves Void sentinel behavior; name ownership and parent-transform emptiness checks are migrated. No tests executed. Evidence: `.cache/native-bevy/evidence/compile-recovery-20260914T234218Z/`.

Raycast/skeleton clone-only source integrated after actual five-line diff review and before-content guards. Isolated overlay based on accepted 1,566-error snapshot reported 1,561 errors; five borrowed Symbol move errors removed, four dynamic-name API errors remain in these two modules. No signature or numerical behavior changes. No tests executed. Evidence: `.cache/native-bevy/evidence/raycast-skeleton-clone-integration/`. Next combined live check will verify integration with the persistent-transform batch.

Combined clone checkpoint `20260914T234411Z`: frozen lib-test no-run compilation exited 101 with 1,559 errors (five fewer), unchanged full source/Cargo hashes after raycast/skeleton integration. No tests executed. Evidence: `.cache/native-bevy/evidence/compile-recovery-20260914T234411Z/`.

Point call source integrated after review: explicit context for call/duplicate/inside/getAt/setAt, checked consumed refs/direct symbols, preserved component flags/index bounds/inside truncation, and synchronous dispatch. ObjCall fixtures cover count, set/get flag behavior, unknown methods and foreign input rejection. Pilot no-run log reported 1,616 errors on its older overlay with no added-region diagnostics, but its manifests span edits rather than compilation, so that frozen-source claim is not accepted; authoritative combined compilation follows the next W3D freeze. Legacy async dispatcher and global inside caller still need explicit context; Point.set_prop remains deferred. Evidence: `.cache/native-bevy/evidence/point-call-integration/`. No tests executed.


- [x] Point/shader-sync combined checkpoint `20260914T235609Z`: frozen locked/offline lib-test no-run exited 101 with 1,556 errors, unchanged source/Cargo hashes. No diagnostics in migrated Point call or shader-sync regions. Shader sync now uses explicit table validation and checked values while retaining pass order and defaults; three legacy renderer callers remain context dependencies. No tests executed. Evidence: `.cache/native-bevy/evidence/compile-recovery-20260914T235609Z/`.
- [x] Review and integrate export_textures readonly-table Result API, authoritative display names and Symbol clone; original bytes/TGA/iteration behavior preserved. Before-content hashes checked.
- [ ] Compile-check integrated export_textures with the next frozen build.

- [x] Review and integrate Rect explicit-context calls and synchronous dispatch; preserve numeric geometry and component flags, validate consumed refs/direct symbols before mutation. ObjCall fixture covers set/get flags and foreign input with unchanged receiver/depth. Frozen isolated older overlay reports 1,616 errors and 277 warnings; full pre/post manifests match. No tests executed. Evidence: `.cache/native-bevy/evidence/rect-call-integration/`.
- [ ] Verify Rect and texture exports in frozen combined source. Legacy Rect async/global inflate callers and set_prop remain dependencies.

- [x] Rect/texture-export combined checkpoint `20260914T235845Z`: frozen locked/offline lib-test no-run exits 101 with 1,554 errors (two fewer), unchanged full source/Cargo hashes. No tests executed. Evidence: `.cache/native-bevy/evidence/compile-recovery-20260914T235845Z/`.

- [x] Transform-cache checkpoint `20260915T000542Z`: reviewed explicit readonly-table get_or_init_node_transform Result chain, selected cached-handle/direct-symbol checks, preserved Void/nonfinite fallback, canonical identity and first-match lower/display fallback. Translation/rotation/scale validate receiver before flushing. Frozen locked/offline lib-test no-run exits 101 with 1,552 errors (two fewer), unchanged full hashes and no diagnostics in changed helper/callers. No tests executed. Evidence: `.cache/native-bevy/evidence/compile-recovery-20260915T000542Z/`.


### Broader compiler-recovery passes

User requested larger batches rather than micro-increments. Baseline: frozen `20260915T000542Z`, 1,552 errors. Retain behavioral/ownership review and isolated file ownership, but batch related modules and run combined checks.

- [ ] Recover remaining W3D object call-body symbol and ownership errors in one pass.
- [ ] Recover related synchronous scalar/geometry datum handler families and explicit dispatcher routes together.
- [ ] Complete W3D OBJ/MTL export and material resolver family.
- [ ] Migrate static evaluator recursion and wrappers as one coherent pass.
- [ ] Review combined diffs and compile frozen integrated source; execute tests once compilation succeeds.

- [x] Review/integrate OBJ/MTL serializers and material resolver chain with explicit readonly tables, fallible names, preserved identity lookup and lazy fallback. Exact before/after content guards checked. Pilot no-run log: 1,549 errors vs 1,552 baseline (9 removed, 6 deferred host-call/Result errors added). Manifests establish only types.rs changed across edits, not an unchanged compile boundary; frozen combined verification remains pending. No tests executed. Evidence: `.cache/native-bevy/evidence/obj-mtl-export-integration/`.
- [ ] Recover synchronous Shockwave3D cast-member module as a larger batch (135 baseline diagnostics); prioritize ahead of static evaluator.

- [x] Review/integrate nine-file synchronous datum batch: Vector/Symbol/Color/Math/Date calls and dispatcher routes, Point/Rect explicit-table setters, focused ObjCall fixtures. Preserves aliases/numeric formulas, Math filtering, browser Date construction, and setter conversion order. Vector fixture now uses a local outer list with foreign component to reach nested validation. Frozen isolated no-run reports 1,549 errors with matching manifests; exact before-content and final manifest guards checked on integration. No tests executed; Date native execution remains unavailable pending browser abstraction. Combined check waits for broader W3D freeze. Evidence: `.cache/native-bevy/evidence/datum-call-family-integration/`.

Broader-batch diagnostic checkpoint `20260915T004211Z`: frozen combined locked/offline lib-test no-run reports 1,491 errors vs preceding 1,552 (61 fewer), unchanged full manifests. Includes export and nine-file datum integration plus W3D call-body WIP. W3D still has seven primary type diagnostics and outstanding navigator lookup/identity corrections; this is diagnostic evidence, not accepted W3D completion. No tests executed. Evidence: `.cache/native-bevy/evidence/compile-recovery-20260915T004211Z/`.

Broader-batch diagnostic checkpoint `20260915T004817Z`: frozen combined locked/offline lib-test no-run reports 1,488 errors, unchanged full manifests. Lookup corrections are applied; four W3D call-body Symbol move errors remain and are assigned together. Export/datum call integration has no new diagnostics in migrated regions. Evaluator and cast-member W3D batches remain staged under behavioral review. No tests executed. Evidence: `.cache/native-bevy/evidence/compile-recovery-20260915T004817Z/`.

- [x] W3D object call-body source recovery reviewed and compile-checked: frozen `20260915T010151Z` locked/offline lib-test no-run reports 1,484 errors, unchanged manifests, zero diagnostics in shockwave3d_object.rs. Four final Symbol clones preserve motion update order. This is 68 fewer than the broader-pass baseline of 1,552. No tests executed; contextless legacy callers and cast-member/support migrations remain. Evidence: `.cache/native-bevy/evidence/compile-recovery-20260915T010151Z/`.
- [ ] Finish reviewed staged evaluator and cast-member W3D batches; migrate the related W3D support family, then verify their combined integration.

- [x] Review and integrate static evaluator recursion, explicit player/table wrappers, checked consumed datum values, and typed ordinary lookup fallback preserving static/global ordering and debugger behavior. Parser remains pure using stable builtin chunk identifiers. Added nested/arithmetic/foreign-global/unknown-identifier fixtures. Exact before-content and final SHA guards checked; full combined compile is pending. No tests executed. Evidence: `.cache/native-bevy/evidence/static-eval-integration/`.

- [x] Review/integrate synchronous cast-member W3D module recovery (text helpers, getters/setters, full calls and collection helpers). Explicit symbol context, checked consumed datum references, preserved lookup/clone/delete/texture/default behavior, fallible resource updates. Root reviewed final ownership-error conversions, event receiver validation and texture pre-read corrections; exact before-content/hash guards and parse check passed. Async loadFile and renderer helpers remain dependencies. Combined compile pending; no tests executed. Evidence: `.cache/native-bevy/evidence/cast-member-w3d-integration/`.

Combined evaluator/cast-member checkpoint `20260915T012125Z`: frozen locked/offline lib-test no-run exits 101 with 1,342 errors, unchanged full manifests (142 fewer than 1,484). No diagnostics in migrated synchronous evaluator/test regions. Cast-member W3D has two remaining Symbol move errors in synchronous clone code plus two deferred async-load errors. Its object-helper caller needs the newly added table argument/Result handling, assigned to the support batch. No tests executed. Evidence: `.cache/native-bevy/evidence/compile-recovery-20260915T012125Z/`.

- [x] Review/integrate seven-file W3D support family: GLB export, scene merge, skeleton selection, raycasts and direct parser/object/cast-member callers. Explicit table/Result propagation; visited-name skeleton checks preserve search order without whole-scene rescans; merge validates incoming names before writes, retaining collision namespaces and same-table identity semantics. Final before/after hashes and parse checks passed. Includes reviewed cast-member two-clone fixes. Browser export and renderer/async callers remain migration dependencies. Combined compile pending; no tests executed. Evidence: `.cache/native-bevy/evidence/w3d-support-integration/`.

- [x] W3D support combined checkpoint `20260915T013310Z`: frozen locked/offline lib-test no-run reports 1,319 errors (23 fewer than 1,342), unchanged full manifests. No diagnostics in director/chunks/w3d support modules or shockwave3d_object.rs; cast-member W3D has three deferred async-loader argument errors. Reviewed source recovery is compile-checked in these regions; full VM remains unbuildable and no runtime tests executed. Evidence: `.cache/native-bevy/evidence/compile-recovery-20260915T013310Z/`.

### Active compiler-recovery families after 1,319-error checkpoint

- [ ] String/StringChunk datum and global handlers, synchronous dispatcher routes and ObjCall fixtures: initial staged diff reviewed; checked source-chain access and typed invalid-reference propagation through `.value` fallback require correction before integration.
- [ ] Cast-member leaf family: button, field, font, text and vector-shape explicit context migration, with available-context callers.
- [ ] WebGL 3D renderer: propose and migrate symbol-context errors while preserving rendering behavior and recording remaining browser-owner dependencies.
- [ ] Review these complete family batches, integrate with exact source guards, and run frozen combined lib-test compilation. Full runtime tests remain unexecuted until compilation succeeds.

Navigator review of the staged five-leaf cast-member family: full 2,002-line diff reviewed; integration withheld pending one coherent correction pass for remaining removed Symbol APIs, missing helper context, typed foreign-reference errors, and checked consumed nested values. Text/style/tab/vertex local conversion defaults and short-circuit order must remain intact. Parse-only evidence is insufficient; the pilot is assigned one frozen family compilation after correction. String fallback classification now includes static-evaluator-reachable comparison, arithmetic and formatting helpers. No new integrated compiler count or runtime result yet.

- [x] Review/integrate eleven-file string family: explicit String/StringChunk dispatch, global string context, checked consumed source/style/list references, typed InvalidReference propagation through static `.value` fallback, and arithmetic/comparison/formatting checks. ObjCall and nested foreign-symbol fixtures included; ordinary invalid-input fallback retained. Exact before/after hashes checked. Full frozen combined compile pending; runtime tests not executed. Evidence: `.cache/native-bevy/evidence/string-family-integration/`.

String-family checkpoint `20260915T020013Z`: frozen locked/offline lib-test no-run reports 1,299 errors (20 fewer than 1,319), unchanged full source/Cargo manifests. Exactly one diagnostic overlaps changed ranges: new driver fixture uses unwrap_err on Result<Datum, ScriptError>, requiring an unavailable Debug implementation; assigned explicit-match correction. Other migrated string, arithmetic, comparison, formatting and error-classification ranges have no diagnostics. Full VM remains unbuildable; no runtime tests executed. Evidence: `.cache/native-bevy/evidence/compile-recovery-20260915T020013Z/`.

- [x] Review/integrate five cast-member leaves (button, field, font, text, vector shape) plus explicit member-borrow/checked-value helpers. Preserved text/layout/style/tab/vertex behavior and visited conversion defaults; fallible chunk selectors prevent panic regression, SymbolError propagation distinguishes foreign from unsupported local values. Exact before/after guards checked. Included reviewed driver fixture explicit Err match replacing Debug-bound unwrap_err. Combined frozen compile pending. Evidence: `.cache/native-bevy/evidence/cast-leaf-integration/`.

- [x] Cast-leaf combined checkpoint `20260915T020831Z`: frozen locked/offline lib-test no-run reports 1,227 errors (72 fewer than 1,299; 92 fewer than the preceding 1,319-error active-family baseline), unchanged full source/Cargo manifests. No errors in the five leaf modules or any changed integration ranges; driver fixture Debug-bound error resolved. Full VM remains unbuildable and runtime tests remain unexecuted. Evidence: `.cache/native-bevy/evidence/compile-recovery-20260915T020831Z/`.

### Next active compiler families

- [ ] Complete and review staged WebGL scene3d renderer family, preserving symbol identity/interning and lazy ownership/error behavior.
- [ ] Recover synchronous TypeHandlers/TypeUtils in types.rs under approved explicit-context scope; async value/new/host boundaries remain excluded.
- [ ] Inspect/propose synchronous PhysX cast/object/native helper family; implementation waits for navigator approval.
- [ ] Integrate reviewed families and run frozen combined compilation; execute full runtime tests once the VM compiles.

Navigator renderer review: full second staged delta (1,633 lines) reviewed. One correction batch assigned for trimmed resource-name interning, motion comparison fidelity, optional GPU setup failures, checked shader/material lookup chains, lazy parent/default validation, and render-target name ownership. Local renderer compile success is not accepted integration evidence until these changes are reviewed. PhysX three-module proposal approved with direct-context callers only.

- [ ] After synchronous type/handler recovery, replace canonical driver ExtCall placeholder name/empty args with real stack decoding and context-aware global routing. Preserve legacy birth/new, first-argument script, movie/frame/global/stage, virtual, async-Xtra and builtin precedence, plus scope result/return/depth behavior; do not bypass script overrides to reach migrated builtins. Source pointers: driver.rs prepare_internal_action and player/mod.rs player_call_global_handler/player_ext_call.

Navigator family review continuation: renderer third/fourth deltas reviewed in full; corrected Symbol identity versus string comparison and restored original empty-name material lookup branches, pending final frozen handoff. Entire first staged TypeHandlers delta (1,217 lines) reviewed; assigned one correction batch for lazy matrix/descriptor ownership checks, original ilk interning, retained collection references, and explicit script-instance getter context. PhysX native solver delta reviewed; fallible display-name ordering required instead of silently treating foreign names as empty strings. No new integrated compiler count or runtime-test claim.

- [x] Review/integrate scene3d renderer family: explicit authoritative table throughout rendering lookup/interning chains, foreign-symbol errors propagated, original identity/display/trim behavior and optional GPU setup preserved. Exact before/after source guards matched reviewed final snapshot. Combined frozen native compilation pending; frontend table-owner caller remains explicit dependency. Evidence: `.cache/native-bevy/evidence/renderer-family-integration/`.

- [x] Renderer combined checkpoint `20260915T022705Z`: frozen locked/offline native lib-test no-run reports 1,121 errors (106 fewer than 1,227), unchanged full source/Cargo manifests. Zero errors with any renderer source span and zero changed-range errors. Full VM remains unbuildable; runtime tests unexecuted. Evidence: `.cache/native-bevy/evidence/compile-recovery-20260915T022705Z/`.

- [ ] Havok synchronous four-file family approved: member/object handlers, HKE parser table ownership, pure physics Symbol clones. Shared script/cast-member callers remain coordinated dependencies.

Navigator PhysX review: complete first member/object/caller diffs reviewed (1,202 + 686 + 20 lines); corrections assigned for removing compiler-silencing always-error legacy wrappers, fallible visited scene-symbol lookups, retained callback instance ownership, preserved diagnostics, and checked selected state names. Solver math remains unchanged in reviewed delta. TypeHandlers second/third corrections reviewed; final frozen handoff pending.

- [x] Review/integrate synchronous TypeHandlers/TypeUtils family: explicit context, checked consumed values and returned property refs, preserved conversion/matrix/descriptor fallbacks, retained-list/filter ownership, original ilk interning, explicit script-instance getter. Full reviewed source guards matched. Manager/contextless callers and excluded async/host bodies remain dependencies. Runtime fixtures still pending; combined compilation pending. Evidence: `.cache/native-bevy/evidence/types-family-integration/`.

- [x] Types combined checkpoint `20260915T023243Z`: frozen locked/offline native lib-test no-run reports 1,146 errors, unchanged full source/Cargo manifests. Types primary errors fell from 36 to 9 (all excluded async/host bodies); zero errors overlap changed ranges. New required-context signatures expose 52 old caller errors (49 manager, 3 script/script-instance/player); net +25 versus 1,121 renderer checkpoint, still 81 fewer than prior 1,227. Synchronous builtin-manager/caller family is next; no runtime tests executed. Evidence: `.cache/native-bevy/evidence/compile-recovery-20260915T023243Z/`.

- [ ] Builtin-manager family proposal approved: explicit synchronous dispatcher and player-state helpers, existing String/Types/List/PropList routes, and focused TypeHandlers ownership/fallback fixtures. Preserve global collection special cases: count(VOID), named property keys, XML zero-based indexing, and loose Symbol/String membership equality. Host/async and global script precedence remain explicit boundaries.

Navigator PhysX correction review: native solver Result propagation accepted; legacy error stubs removed. Reviewed subsequent member/object deltas and required replacing eager scene validation with visited-only fallible loops, retaining supplied callback VOID, restoring original lower-versus-display comparisons, and validating selected generated object names. Final integration remains pending this complete correction batch and frozen diagnostic evidence.

- [x] Review/integrate five-file synchronous PhysX family: explicit member/object context, checked visited refs and scene names, typed native collision-pair lookup errors, preserved solver math and supplied callback VOID, validated retained callback instance ownership. Shared getter callers carry the authoritative table; contextless call/set routes remain dependencies. Exact before/after source guards matched reviewed root freeze. Combined frozen compilation pending. Evidence: `.cache/native-bevy/evidence/physx-family-integration/`.

- [x] PhysX combined checkpoint `20260915T024643Z`: frozen locked/offline native lib-test no-run reports 1,077 errors (69 fewer than 1,146), unchanged full source/Cargo manifests. Zero primary errors in three PhysX implementation modules and zero errors overlapping changed ranges across all five files. Full VM remains unbuildable; runtime tests unexecuted. Evidence: `.cache/native-bevy/evidence/compile-recovery-20260915T024643Z/`.

Navigator full-family reviews: Havok member/object/HKE/physics first staged deltas reviewed (994/511/112/29 lines); assigned one correction batch for checked consumed values and callbacks, original display comparisons, optional shape fallback, and outgoing state names. Builtin-manager first delta reviewed (1,104 lines); global getLast/getAt/setAt semantics must survive dispatch, zero-argument inflate fallback retained, synchronous inline state accesses included. Bitmap four-file proposal approved with no pixel algorithm changes.

Canonical driver follow-up evidence: prepare_internal_action still supplies placeholder values for NewObj, ExtCall, ObjCallV4, SetObjProp, and TellCall. ExtCall must decode actual current opcode name id and pop_call_args, retain no-return metadata through pending completion, and apply the result exactly once; legacy flow_control::ext_call lines113ff is the reference. This remains required beyond recovering builtin-manager signatures.


### Resumed broad recovery batches

- [ ] Complete staged Havok callback/reference corrections and six-file narrow caller handoff; navigator integrates after diff review.
- [ ] Complete staged builtin-manager synchronous dispatch and TypeHandlers fixtures, preserving global collection edge cases.
- [ ] Complete staged bitmap/member bitmap and low-level symbol-context family with unchanged pixel algorithms.
- [ ] Recover the next coherent cast-member dispatcher family after those signatures settle: synchronous call/get/set helpers and direct script callers. Current frozen baseline has 83 primary errors in cast_member_ref.rs. Preserve erased/invalid-member no-op writes, version-dependent member numbering, preload/unload no-ops, text-to-W3D fallback classification, member-name cache invalidation and notifications. Interning getters require the authoritative mutable table; do not substitute a new interner.
- [ ] Run frozen combined compilation after reviewed integration and report actual count; full runtime tests remain blocked by compilation.

- [ ] Resolve raw bitmap handle boundary ownership before session-isolation acceptance: BitmapRef remains u32 and BitmapManager lookups validate existence only. Checked owning DatumRef access protects the current handler route, but separately copied raw IDs can numerically collide across managers. Qualify externally retained/transferred bitmap values or reject them at boundaries; add a same-ID cross-session regression fixture. Four-file compiler recovery does not establish this guarantee.

Navigator resumed review: Havok fourth delta (272 member + 56 object lines) reviewed; remaining corrections preserve lazy persistent-transform fallback, original display/lower comparison operands, and validation only for visited collision-name comparisons. Manager second delta (857 lines) plus TypeHandlers fixture delta (162 lines) reviewed; correction batch covers named getAt conversion order, checked consumed references in newly migrated helpers, VOID classification, zero-argument inflate, and flat newMatrix fixture semantics. Bitmap palette helper must preserve local unknown-name None while rejecting foreign symbols. No new integration or compiler-count claim yet.

- [x] Review/integrate six-file Havok family: explicit context, checked consumed references, ordinary callback values retained, lazy transform fallbacks, validated copied/generated names, preserved comparison operands and solver math. Exact six-file before/after SHA guards matched reviewed sources. Staged native lib-test compile had zero owned primary errors; authoritative combined compile pending. Evidence: `.cache/native-bevy/evidence/havok-family-integration/`.

- [x] Havok combined checkpoint `20260915T033553Z`: frozen locked/offline native lib-test no-run reports 976 errors (101 fewer than 1,077), unchanged full source/Cargo manifests. Owned family primary errors: 0; changed-range errors across six files: 0. Full VM remains unbuildable; runtime tests unexecuted. Evidence: `.cache/native-bevy/evidence/compile-recovery-20260915T033553Z/`.

- [ ] Next cast-member dispatch batch approved with four owned files: cast_member_ref.rs, narrow script.rs getters, sound member and palette member. Preserve all prior dispatch/no-op/fallback behavior; coordinate manager chunk setter and bitmap APIs.
Navigator bitmap review: full datum delta982lines and member delta282lines reviewed. Required correction pass propagates foreign-reference errors through draw-thickness alias fallback and stage-dirty lookup, validates consumed owned setter values, and resolves symbolic ink fallibly before numeric compositor calls. All eight non-Lingo copy_pixels callers inspected: their ink maps are numeric or empty. Pixel algorithms remain unchanged.
Navigator manager final handoff review: staged compile reports19 primary diagnostics across manager/types, chiefly excluded async/host bodies plus pending bitmap/castmember signatures; no fixture errors reported. Broad rustfmt churn introduced at handoff must be removed by restoring reviewed fourth snapshots and reapplying only final semantic changes before integration.

- [x] Review/integrate builtin-manager synchronous context dispatch and TypeHandlers ownership/fallback fixtures. Preserved global collection special cases, named lookup conversion order, empty getLast and inflate paths, checked consumed values, modifier/input state, chunk fallback classification and ignored-extra behavior. Narrow xtra factory context and sound Symbol clone included. Exact before/after hashes and full reviewed snapshots matched; pending bitmap/cast setter signatures and excluded host/async code remain. Combined frozen compilation pending; runtime fixtures not executed. Evidence: `.cache/native-bevy/evidence/manager-family-integration/`.

- [x] Manager combined checkpoint `20260915T035355Z`: frozen locked/offline native lib-test no-run reports857 errors (119 fewer than976;220 fewer than resumed1077baseline), unchanged full source/Cargo manifests. Seventeen manager/types primary diagnostics remain: eight manager import/async/host, seven excluded TypeHandlers async/host, and two planned bitmap/castsetter API dependencies. Three overlap changed ranges: the existing unresolved script-handler import on the expanded import line and those two caller signatures. No fixture-specific errors; runtime fixtures remain unexecuted. Evidence: `.cache/native-bevy/evidence/compile-recovery-20260915T035355Z/`.
- [ ] Next synchronous movie/sprite family assigned for proposal: handlers/movie.rs, datum_handlers/sprite.rs, narrow manager caller wiring; async scheduling and host boundaries remain explicit dependencies.

- [x] Review/integrate four-file bitmap family: explicit player/table context, checked consumed bitmap/datum references and nested masks, preserved local palette/thickness fallbacks, fallible symbolic ink resolution before numeric compositor. All non-Lingo copy_pixels callers audited as numeric/empty; pixel algorithms unchanged. Exact before/after SHA guards and reviewed snapshots matched. Raw u32 bitmap ownership remains a separate boundary gap. Combined compilation pending. Evidence: `.cache/native-bevy/evidence/bitmap-family-integration/`.

- [x] Bitmap combined checkpoint `20260915T040044Z`: frozen locked/offline native lib-test no-run reports834 errors (23 fewer than857;243 fewer than resumed1077baseline), unchanged full source/Cargo manifests. Four-file primary errors: 0; changed-range errors: 0. Manager bitmap forwarding arity dependency now resolved; cast dispatcher/property callers remain active. Runtime tests remain unexecuted. Evidence: `.cache/native-bevy/evidence/compile-recovery-20260915T040044Z/`.
- [ ] Synchronous script/script-instance datum family assigned for proposal, including property/reflection/lookup preparation; actual async invocation remains driver-owned boundary.

Navigator cast-dispatch first review completed: full1262-line cast-member delta plus123-line sound,51-line palette,11-line script deltas. Correction batch requires removing out-of-scope ExecutionContext borrows across async waits and undefined async setter caller context; preserving typed InvalidReference through ordinary member fallbacks; checked consumed move/duplicate args and owned setter values; original symbol comparison/interning semantics; and direct bitmap pixel proxy behavior. No integration yet.
- [ ] Synchronous script/script-instance family explicitly approved in `/private/tmp/dirplayer-script-datum-stage-20260915`; async constructors/invocation remain driver dependencies.

Navigator resumed family review: inspected complete movie/sprite/manager caller deltas (304/389/84 lines), plus in-progress script/script-instance deltas (198/538 lines). Assigned coherent correction batches for fallible camera-name lookup, lazy consumed argument validation, typed ownership errors through property fallback, retained instance/reference validation, preserved case-sensitive string matching, and constructor ownership. No additional integration or compiler-count claim; last authoritative frozen checkpoint remains 834 errors.
- [ ] Review corrected movie/sprite family and script datum family, coordinate shared player/script.rs helper ownership, then integrate with exact source guards and run frozen combined compilation.

- [x] Review/integrate three-file synchronous cast-member dispatch, sound and palette family. Preserved pixel proxy behavior, local fallbacks, no-op branches, symbol comparison/interning and notifications; propagated consumed-reference ownership failures. Exact baseline and reviewed snapshot hashes matched. Stage reports only deferred async Havok callback arity diagnostic within owned files; authoritative combined compilation pending. Evidence: `.cache/native-bevy/evidence/cast-dispatch-integration/`.

- [x] Cast dispatcher combined checkpoint `20260915T041849Z`: frozen locked/offline native lib-test no-run reports 744 errors (90 fewer than 834), unchanged full source/Cargo manifests. Zero changed-range errors across three integrated files; one owned primary diagnostic remains in deferred async Havok step callback invocation. Full VM still unbuildable, runtime tests unexecuted. Evidence: `.cache/native-bevy/evidence/compile-recovery-20260915T041849Z/`.

Navigator post-checkpoint review: movie/sprite second and third deltas (38/488 and 10/229 lines) reviewed; property fallbacks now preserve typed ownership failures and original argument evaluation order. Final acceptance still requires the requested regression fixtures and a fresh live-source compile overlay; the pilot's stale-stage 1,010-diagnostic report is not a new authoritative checkpoint. Script-datum review rejected unsolicited StaticDatum property-default initialization and restored original VOID initialization; constructor/setter ownership and ancestor error preservation remain under review.
- [ ] Central datum dispatcher mod.rs proposal approved: connect migrated bitmap/cast/sprite/Havok/PhysX synchronous routes; explicitly defer async cast operations and non-migrated sprite/host/script paths, preserve recursion-depth accounting and typed ownership errors.

- [x] Review/integrate central synchronous datum routing for bitmap/cast/sprite/Havok/PhysX, with explicit async classification, typed ownership errors and depth cleanup. Fixtures cover dynamic foreign handler/receiver, deferred import/load and unknown sprite handlers. Exact source guards matched; staged compile reports745 errors with only changed-range dependency being pending Sprite::call signature. Combined frozen compilation pending; runtime fixtures unexecuted. Evidence: `.cache/native-bevy/evidence/datum-dispatch-integration/`.

- [x] Datum dispatcher combined checkpoint `20260915T043755Z`: frozen locked/offline native lib-test no-run reports745 errors, unchanged full source/Cargo manifests. One additional changed-range diagnostic is the pending Sprite::call context signature; other dispatcher diagnostics remain excluded legacy async routes. Runtime fixtures unexecuted. Evidence: `.cache/native-bevy/evidence/compile-recovery-20260915T043755Z/`.
- [ ] Canonical driver internal opcode preparation proposal assigned: replace placeholder NewObj/ExtCall/ObjCallV4/SetObjProp/TellCall operands with real names, stack values, target identity and no-return metadata, preserving exactly-once completion and precedence.

- [x] Review/integrate three-file synchronous Movie/Sprite family and manager caller wiring. Preserved member sentinels/early returns, sprite property precedence, consumed argument order, camera defaults and typed ownership errors. Reviewed scene-default foreign-camera and ignored-cast-argument fixtures; staged native compile has no fixture diagnostics. Exact before/after guards matched. Pending script setter signature and legacy host/async paths remain dependencies; combined frozen compilation pending. Evidence: `.cache/native-bevy/evidence/movie-sprite-integration/`.

- [x] Movie/Sprite combined checkpoint `20260915T044156Z`: frozen locked/offline native lib-test no-run reports726 errors (19 fewer than745), unchanged full source/Cargo manifests. Dispatcher Sprite signature resolved; only changed-range error is pending script_set_prop table signature. No fixture errors. Full VM remains unbuildable; runtime tests unexecuted. Evidence: `.cache/native-bevy/evidence/compile-recovery-20260915T044156Z/`.
- [ ] Score property helper family assigned for proposal: explicit sprite getter/setter and synchronous helper chain, coordinated narrow callers; lifecycle async scope separate.

Navigator script-family final review: complete fourth deltas reviewed (script88/instance159/player_script189 lines). Integration withheld for a concrete foreign-ancestor bug: script_set_prop required=false local-property fallback still swallowed InvalidReference. Assigned one final correction batch for that guard/regression, lazy diagnostic formatting, ancestor validation before outgoing allocation, original selected-property/value evaluation order, and promised ancestor ownership/VOID fixtures. No new script-family integration claim.
- [ ] Score property family approved: explicit player/table through sprite getter/setter and synchronous helpers, with narrow sprite/manager callers; player/script.rs remains separately owned until script-family release.
- [ ] Driver decode proposal approved with opcode-specific completion policies: NewObj always pushes, SetObjProp preserves last result, ExtCall updates scope return and handles return/Stop, ObjCallV4 preserves global precedence, Tell retains owner-qualified target.

- [x] Review/integrate three-file synchronous script/script-instance datum and script helper family. Authoritative symbols initialize original VOID properties; checked retained refs/ancestor handles, static/instance property helpers, reflection and virtual lookup preserve precedence and typed errors. Regression covers foreign same-ID ancestor local-fallback rejection and VOID no-detach; no fixture diagnostics. Exact source guards matched. Staged705-error compile has one changed-range diagnostic on an expanded import line for the existing missing async script-handler function; remaining errors are excluded paths/callers. Timeout delegation remains a documented synchronous ambient dependency. Combined frozen compilation pending. Evidence: `.cache/native-bevy/evidence/script-datum-integration/`.

- [x] Script family combined checkpoint `20260915T045929Z`: frozen locked/offline native lib-test no-run reports705 errors (21 fewer than726), unchanged full source/Cargo manifests. Sprite script-setter dependency resolved; only changed-range error is existing missing player_call_script_handler import on an expanded import line. No fixture diagnostics. Full VM remains unbuildable; runtime tests unexecuted. Evidence: `.cache/native-bevy/evidence/compile-recovery-20260915T045929Z/`.
- [ ] Synchronous timeout family approved: explicit call/set/get/forget and narrow instance/dispatcher routing; preserve ignored ancestor values and timer behavior, validate retained targets. Actual timer scheduling/async constructors remain separate ownership dependencies.
Navigator driver first review: full654-line decoder/effect delta reviewed. Required correction batch includes stack-only ObjCallV4 name resolution/pop order, validated completion results before mutations, opcode-specific Return/scope behavior, owner-qualified nested targets, inner-instance checks, and borrow-safe local decode helpers.

Navigator continued batch review: read complete second driver delta and second score delta (364 lines), plus first three-file timeout delta. Driver corrections require legacy ObjCallV4 stack-pop order, typed completion ownership failures, and consistent forced-return handling. Score corrections preserve unconditional successful member notification after conditional filmloop reset and validate retained instance handles before no-op equality. Timeout corrections preserve ignored ancestor receiver/value and unscheduled-target value ordering; allocator inner-instance checks independently verified as owner/generation-aware. All three families remain staged; authoritative checkpoint remains705 errors, no new runtime-test claim.

Navigator follow-up: reviewed third driver delta including313lines of opcode fixtures, and third score delta126lines. Required fixes include removing the invalid SetObjProp fixture argument marker, preserving consumed Symbol comparison operands/invalid-sprite discard order, and validating visited existing instance handles before no-op equality. Independently verified RuntimeSession::with_player does not enforce OwnerToken liveness; Tell capture/resume must check liveness in addition to identity. Timeout fixtures must use foreign ignored values and locally wrapped foreign instance handles to exercise the claimed boundaries. No family integrated during this review; frozen baseline remains705.

- [x] Review/integrate three-file synchronous timeout family: explicit player/table call, setter/getter/forget and dispatcher/instance routing; preserve ignored ancestor receiver/value and unscheduled-target evaluation order, check retained inner instance targets. Full reviewed snapshots and exact before/after guards matched; fresh staged native lib-test no-run704 errors, zero changed-range or fixture errors. Timer scheduling/async constructor remains a separate ownership dependency. Combined frozen compilation pending; runtime tests unexecuted. Evidence: `.cache/native-bevy/evidence/timeout-datum-integration/`.

- [x] Timeout combined checkpoint `20260915T052609Z`: frozen locked/offline native lib-test no-run reports704 errors (one fewer than705), unchanged full source/Cargo manifests. Staged reviewed three-file changed ranges and fixtures have zero errors; excluded async/host callers remain. Full VM remains unbuildable; runtime tests unexecuted. Evidence: `.cache/native-bevy/evidence/compile-recovery-20260915T052609Z/`.
- [ ] Next movie/stage property family assigned for proposal: Movie/Stage accessors, DirPlayer movie properties and narrow synchronous bytecode callers; authoritative symbols and checked retained values with original no-op/fallback semantics.

- [x] Review/integrate canonical internal opcode preparation and completion policies for NewObj/ExtCall/ObjCallV4/SetObjProp/TellCall: actual names/stack operands, constructor normalization, no-return/Return/scope effects, fallible global precedence scan, owner-qualified live nested targets and validated retained completion values. Focused fixtures cover opcode policies, malformed markers, duplicate completion, foreign globals/results and reset/replaced targets. Exact reviewed source guards matched; frozen stage matches live except driver, native lib-test compile704 errors with zero driver errors. Actual internal invocation execution remains next dependency; runtime fixtures unexecuted. Combined compile pending. Evidence: `.cache/native-bevy/evidence/driver-internal-integration/`.

- [x] Driver internal combined checkpoint `20260915T053305Z`: frozen locked/offline native lib-test no-run reports704 errors, unchanged full source/Cargo manifests; driver primary errors0. This replaces real opcode placeholders without reducing the existing unrelated diagnostic count. Runtime fixtures remain unexecuted. Evidence: `.cache/native-bevy/evidence/compile-recovery-20260915T053305Z/`.
Navigator score handoff audit: reported690-error log still contains property-path errors at4891 and5451-5460; rejected claim of zero migrated-path errors and withheld integration. Correction batch covers remaining Symbol methods, original lower/display comparisons, consumed raw Datum ownership before no-op truthiness and regression coverage.
- [ ] Session-owned Global internal-call execution assigned for proposal: preserve constructor/script/virtual/async/sync precedence and continuation return handling without ambient access or VM borrows across waits.

- [ ] Movie/stage property implementation approved: associated Movie getter with explicit owning player/table, disjoint setter dependencies, DirPlayer/Stage property paths and narrow synchronous bytecode/static-eval callers. Preserve ignored values and state-dependent getter behavior; remove ambient getter branches without aliasing the owning player.
- [ ] Global internal execution proposal approved with corrections: New skips script lookup but still permits virtual global precedence; first-argument SpriteRef retained; Return uses retained actual value locally; script child and early-return policies preserve scope/last/stack and depth; deferred operations retain explicit requests. Existing setup JS/profiling boundary remains an explicit dependency.
Navigator score correction review: read complete first/second correction deltas and noop helper; replaced proposed eager all-values guard with validation at actual conversion/retention sites. Rotation/Skew/Flip defaults, Loc ignored mismatches, unknown properties without behaviors and Camera default must retain original discard behavior. No score integration or new authoritative count; baseline704.

Navigator movie/stage first review completed: full four-file diffs read (Movie341/Stage107/DirPlayer252/GetSet165 lines). Correction batch removes recursive whole-graph validation (nested VOID/cycles), validates only consumed/retained values, preserves ignored ExitLock/system writes, restores allocator-aware retained instance checks, completes authoritative SymbolTable conversions and mutable getter borrows, and checks returned result handles. Score correction3 reviewed; conversion-site helper now replaces eager all-values guards. No new integration; verified baseline704.

Navigator resumed three Luna High pilots after interruption: score getter/cache ownership, Movie/Stage property completion, and session-owned global dispatch. Independently parsed the prior score live704 overlay: 680 compiler errors, with remaining score diagnostics outside the property block; this is staged evidence only and does not replace the integrated 704 checkpoint. Reviewed the full subsequent 306-line score correction delta, Movie/Stage correction deltas, and partial global driver/session deltas. Final source guards, completed family reviews, and combined compilation remain pending.

- [x] Reviewed and integrated three-file score property family with exact before/after SHA guards: explicit owning player/table getter/setter/helper paths, shallow checked behavior-cache resolution and retained instance ownership, original ignored/default/notification and shared-list semantics. Added foreign cached-list/initial-instance and cache-sharing fixtures. Root fresh source/Cargo-frozen live704-plus-reviewed overlay reports680 compiler errors, zero property-block/new-fixture errors; prior pilot719 log is superseded by this independent overlay. Evidence: `.cache/native-bevy/evidence/score-properties-integration/`. Full runtime tests remain unexecuted; combined live compile pending.

- [x] Score combined checkpoint `20260915T114514Z`: locked/offline native lib-test no-run reports680 errors (24 fewer than704), full source/Cargo manifests unchanged. Evidence: `.cache/native-bevy/evidence/compile-recovery-20260915T114514Z/`. VM remains unbuildable and runtime fixtures remain unexecuted. Premature Movie/Stage pilot live copies were preserved under `/private/tmp/movie-premature-live-recovery-20260915T114600Z` and guarded-restored to pre-batch snapshots; that family is still isolated and unaccepted.

- [ ] Five-function W3D event/tick family approved in events.rs: explicit synchronous runtime preparation for timers, animations, particles, W3D collisions and PhysX collisions; owned requests distinguish instance/static/global targets and retain exact owner identity. Owner-bound clock type replaces static clocks; session storage and scheduler wiring remain coordinated dependencies. Preserve callback ordering/algorithms, no ambient adapters. Current W3D range accounts for34 compiler diagnostics.
Navigator reviewed complete current global execution delta (driver679/session400 lines); required corrections preserve ignored completion and Return effects, bind extra call-depth cleanup to exact live ScopeTokens, preserve empty cache hits, and validate parent before mutations. No new integration; authoritative baseline680.

Global review follow-up: verified legacy player_ext_call Return preserves last_handler_result and local Tell uses the extra call-depth layer; required exact live ScopeToken-bound accounting and per-callback setup-expectation checks before continuing dispatch. Movie review separates direct reference checks from actual retained-list checks to avoid inspecting ignored nested TimeoutList entries. The interim global-stage build reports704 errors and zero driver/session primary diagnostics, but predates latest amendments and uses stale dependencies; it is not authoritative integration evidence. Baseline remains680.

Navigator read complete first W3D event/tick patch802 lines. Required corrections bind clocks to exact owner and preserve integer/sentinel timing and original asymmetric name comparisons; validate reached callbacks before consuming their timer/collision queues and preserve ignored targets. The unchanged persistent-transform flush still consumes thread-local dirty IDs, so explicit collision preparation must receive owner-qualified dirty input and flush it through checked local references. Native dirty-state producer/storage and scheduler wiring remain separate required integration work. Global execution handoff returned for missing central sync/child/virtual/preference/reset fixtures and retained Return validation; its stale704 compile is not final evidence.

- [x] Movie/Stage five-file family reviewed and guarded-integrated, including fallible Globals and same-handle ActorList validation. Frozen combined checkpoint `20260915T122610Z`:661 compiler errors, unchanged source/Cargo manifests; runtime fixtures unexecuted. Evidence: `.cache/native-bevy/evidence/movie-stage-integration/`.
- [ ] Global dispatch driver/session reviewed and guarded-integrated: real synchronous and script-child dispatch, exact-scope depth cleanup, virtual precedence/reset checks, retained Return validation. Combined frozen checkpoint `20260915T122803Z`:662 errors; fresh dependencies expose one new fixture lifetime error at driver.rs3838, correction assigned before verification acceptance. Evidence: `.cache/native-bevy/evidence/global-dispatch-integration/`.
- [ ] W3D correction review completed; remaining corrections: dirty-transform empty-input semantics and validation of matched entries, validation of actually selected PhysX/particle names, and actual rejection/no-consumption fixtures. Family remains isolated.
- [ ] Next coherent compiler families assigned for proposal to Luna High pilots: synchronous evaluator and remaining bytecode helpers.

- [x] Global dispatch final fixture lifetime correction reviewed and guarded-integrated (driver SHA256 `e9d6da116c3a594533fa6081ed743dc6e0c7edc925ea9fadcb20077d72c34625`). Frozen combined checkpoint `20260915T123022Z`:661 errors, unchanged source/Cargo manifests, zero driver/session primary errors. Runtime fixtures remain unexecuted.

- [ ] Canonical evaluator continuation design approved for Luna High implementation in isolated eval/session/driver files: full AST/chunk family, explicit short borrows, exact-owner top-level capability and optional real scope capture, owned pending requests, lazy multiline parsing. Caller transitions in manager/types/events/lib/testing_shared remain coordinated followup; no ambient evaluator adapter or synthetic unsupported-operation errors.

- [ ] Six-file bytecode family approved: remove orphan ambient async handlers/dispatcher superseded by canonical driver; explicit authoritative symbol formatting in expression tracker; mutable-table sprite-rectangle getter propagation. Includes narrow orphan SetObjProp deletion in get_set.rs; no canonical behavior removal.

- [x] Five-function W3D event/tick family reviewed and guarded-integrated (events SHA256 `46915dc23182ccf3aefc267c557627b6e091d6b3a4bf7fe42a852cc702f12618`). Frozen combined checkpoint `20260915T124311Z`:641 errors (20 fewer than661), unchanged source/Cargo manifests. Explicit owner-bound clocks, owned callback requests, checked reached timer/collision targets and explicit dirty input; runtime fixtures unexecuted. Evidence: `.cache/native-bevy/evidence/w3d-events-integration/`.
- [ ] Owner-local transform dirty family approved for isolated Luna High implementation: transform3d/vector complete explicit mutations, DirPlayer dirty state lifecycle, shared checked flush and narrow shockwave3d_object/script/dispatcher/events callers. Session/driver/eval and bytecode remain separately owned.

Navigator dependency audit: evaluator caller transition must include mod.rs script-text execution and preserve debugger evaluation during pause plus deferred Flash callbacks. Dirty-state family expanded narrowly to cast_member/physx.rs two explicit-table flush callers. Renderer flush callers remain a required coordinated followup because draw_frame signatures/traits currently lack SymbolTable; no no-symbol bypass will be introduced. Live source remains frozen641 while all three Luna High pilots are verified running.

- [x] Six-file bytecode cleanup reviewed and guarded-integrated from exact unchanged pilot snapshots. Removed orphan ambient async implementations/dispatcher superseded by canonical driver; explicit dynamic-symbol display and mutable-table sprite rectangle queries. Frozen combined checkpoint `20260915T125914Z`:624 errors (17 fewer than641), full source/Cargo manifests unchanged; zero owned-file primary errors. Runtime tests remain unexecuted. Evidence: `.cache/native-bevy/evidence/bytecode-cleanup-integration/`.

Navigator evaluator draft audit: explicit helper conversions are staged, but the initial EvalContinuation wrapper still awaits the recursive async evaluator and calls ambient global/datum handlers. This is not accepted as the required state machine. Pilot instructed to replace it with synchronous owned frame/value stacks, real pending request capabilities, exact owner/optional scope validation, and resumption without replaying operands. Live checkpoint remains624.

- [ ] Cast library/manager property and cache family approved with narrow get_set/context_vars/movie/script/dispatcher caller propagation; evaluator pilot separately informed of first-argument SymbolTable lookup API. Preserve cache first-writer order and missing-member/ignored-value behavior. Dirty and cast pilots have disjoint isolated script/dispatcher hunks; root will merge serially with guards. Cast FileName preparation must retain its owned request for session scheduling; legacy discarded-request boundary and canonical SetProperty integration remain explicit dependencies.

Navigator continued review: read all eight cast-family diffs and the complete transform/vector correction deltas. Cast corrections require checked consumed handles, propagation of ownership errors through count fallback, native notification outbox use, and lookup/ignored-argument fixtures. Transform corrections still require complete vector setter propagation and receiver/argument ordering preservation. Renderer dependency audit includes draw_frame, capture_stage_bitmap, draw_sprite_isolated, offscreen nested rendering, and browser draw scheduling; propagating only the two flush calls would leave behavioral gaps. No new source integration or runtime-test claim; verified checkpoint remains624.

Navigator evaluator branch audit: reviewed apply_frame against original runtime evaluator. Required corrections preserve per-operation completion capabilities, prepared child routing without callback replay, breakpoint receiver dispatch, PropList nonnumeric keys and indexed/chunk access variants, indexed/local assignments, same-handle put-into, concat formatting, put-display continuation, and original delete-chunk evaluation order. Simplified replacements are not accepted; original branch behavior must be carried through owned frames. Transform vector followup must resolve reached parent sub-property before writes using authoritative Unicode lower semantics. All work remains staged; checkpoint624 unchanged.

Navigator second continuation review: reviewed complete cast review2 and dirty review3 snapshot deltas. Cast checked consumed-reference paths and optional notification outbox are present; strengthened foreign-symbol/ignored-extra/outbox fixtures requested. Vector setter now prepares selected parent property through authoritative lower before writes and preserves value-before-property error order. New parent/finite-flush fixtures reviewed; caught invalid vector-to-float assertion and fixture import/borrow issues before acceptance. Evaluator pilot has a live native test-no-run process (observed PID23678); all three collaboration handles remain running. No new live source integration; checkpoint624 remains authoritative.

- [x] Reviewed and guarded-integrated owner-local dirty Transform3d family (seven changed files; script unchanged), using exact revision4 snapshots and excluding rejected post-handoff ordering changes. Shared checked flush, checked consumed arguments, vector parent writeback and init/reset lifecycle are integrated. Frozen combined checkpoint `20260915T134223Z`:620 errors (four fewer than624), unchanged full source/Cargo manifests. Runtime fixtures remain unexecuted; legacy setters/dispatcher and renderer callers remain explicit dependencies. Evidence: `.cache/native-bevy/evidence/transform-dirty-integration/`.

- [x] Cast library/cache family reviewed and guarded-integrated across eight files, with disjoint dispatcher merge preserving Transform3d changes. Explicit tables, checked consumed handles, original cache first-writer order, optional Name notification outbox, and foreign/missing-member/ignored-extra fixtures included. Frozen combined checkpoint `20260915T134428Z`:611 errors (nine fewer than620), unchanged full source/Cargo manifests. Legacy FileName request scheduling remains incomplete; runtime fixtures unexecuted. Evidence: `.cache/native-bevy/evidence/cast-cache-integration/`.

- [ ] Next renderer family assigned for proposal: explicit table/error propagation through draw/capture/isolated/nested paths and callers, with host FBO/cache/palette/projection restoration on errors.
- [ ] Next Multiuser/Curl family assigned for proposal: synchronous helpers plus owner-local manager and qualified-handle/callback boundaries; current raw XtraInstanceId=u32 and process-global managers require explicit design.
Navigator evaluator followup: distinct EvalAction and prepared child are now staged. Remaining corrections include current-player/selected-scope validation before turns, returned-handle validation before consuming completion, and non-short-circuit AND/OR conversions. Live source verified identical to frozen611 checkpoint.

- [ ] Renderer explicit core proposal approved for isolated implementation in rendering_gpu/mod.rs, rendering.rs, rendering_gpu/webgl2/mod.rs, stage.rs, and script.rs Stage getter arm. Mutable authoritative table is necessary because shader sync interns strings. Require fallible draw/capture/isolated APIs, actual saved framebuffer/viewport and cache/projection restoration on errors, and Stage Image error propagation without CPU fallback on failure. Legacy scheduler/nested/browser callers remain coordinated dependencies; runtime browser proof cannot be claimed from compile-only coverage.

- [ ] Multiuser/Curl proposal approved for isolated implementation: player-owned managers and lifecycle, checked canonical synchronous routing, owned network/teardown intents with exact owner and instance lifetime. Files: Xtra modules/manager plus narrow player fields/reset, handlers manager/types, datum dispatcher, testing global init. Session/driver remain separately owned. Naked IDs cannot authorize canonical calls or late completions; raw Datum Xtra inner-ID representation gap must remain explicit unless actually qualified. Full network executor followup remains required.

Navigator evaluator final-handoff review withheld integration. Saved frozen snapshots `/private/tmp/eval-root-finalreview1-{eval,session,driver}.rs`; read full session/driver diffs and owned frame engine. Old public async entrypoints still execute ambient recursive evaluator, violating replacement requirement. Required same-family correction: remove superseded recursion, owned lazy command continuation; active selected-scope and current-owner completion validation; completed-eval cleanup; retain global pending dispatch reason; preserve delete operand stack order, conversion timing, global-only chunk-source reads, W3D indexed writeback/fallback, clamped write indices, compound properties, missing-member sentinels, chunk fallback, and concat order. Staged544-error result is not integrated/accepted; live611 remains authoritative.

Navigator renderer handoff review: all five full diffs read (115/107/229/69/11 lines), snapshots `/private/tmp/renderer-root-review1-{0..4}.rs`. Final corrections required before integration: reject failed GL binding/viewport queries before state changes instead of default substitution; propagate both touched readback failures with nested restoration first and restore capture read binding. Explicit core propagation and Stage error handling otherwise aligned. Browser failure-restoration execution remains unverified; live611 unchanged. Evaluator shared helper review also requires validation of actually returned local-container foreign element handles without scanning unrelated entries.

- [x] Renderer core five-file batch reviewed and guarded-integrated: explicit symbol/error APIs, checked framebuffer/viewport queries, readback error propagation and nested GL/cache restoration, Stage Image error propagation. Frozen combined checkpoint `20260915T140639Z`:615 errors (four more than611 as explicit APIs expose legacy callers), unchanged full source/Cargo manifests; zero changed-range primary errors. Browser restoration execution remains unverified. Evidence: `.cache/native-bevy/evidence/renderer-core-integration/`.
- [ ] Renderer caller/context migration assigned for read-only proposal; evaluator canonical replacement and Multiuser/Curl ownership batches remain isolated. Evaluator draft now removes recursive async entrypoints; pending review corrections include preserving continuation on rejected complete_eval and rejecting invalid selected debugger scope without top-level fallback.

Navigator next-family dependency audit: `script.rs::player_set_obj_prop` remains the ambient setter used only by the old evaluator in-tree; canonical driver/evaluator emit owned SetProperty requests but no complete setter executor exists yet. Followup must transfer all setter branches to explicit context, preserve Void/Null ignored-value behavior and conversion order, and retain CastLib FileName load requests for real session scheduling. Do not merely delete the ambient function to reduce its diagnostics. LeechProtection has an independent explicit-context conversion opportunity after Xtra manager ownership lands; retain environment override persistence, external-param insertion order, empty-name short circuit, and existing test coverage. Live source remains identical to the615 checkpoint.

- [ ] Approved isolated internal WebGL helper closure in webgl2/mod.rs: mutable table and Result through render_sprite and both draw callers, authoritative symbol resolution, scene pass propagation, preserved skip/pass/camera order. Full frontend/session/player-graph caller migration remains a separate required boundary.
Navigator preliminary Multiuser/Curl review requires retaining pending intents rather than converting them to synthetic errors, checked receiver authority, real teardown on close/reset in correct collection order, prepared consumed network inputs, full retained callback validation, original Curl constructor conversion fallback and exec fallback, and original Multiuser Unicode dispatch normalization. Batch remains isolated; no acceptance claim.

Navigator evaluator correction review: rejected-completion preservation is now present in the isolated draft; selected-scope bounds added, with original None-to-top default restoration requested. The new local-container foreign-element fixture exposes a remaining selected-result validation gap in ApplyListAccess (container/index checks do not validate the returned element); correction and a central direct-value stack retention check requested. No new source integration;615 remains verified.

Navigator evaluator second full handoff withheld: snapshots `/private/tmp/eval-root-review2-{eval,session,driver}.rs` (eval hash2bea86e5). Staged543 compiler errors and zero owned errors do not prove behavior. Found root pending result discarded by empty-frame turn, invalid repeated turn consuming pending evaluator, put-chunk captured value incorrectly popped again, delayed index conversions, missing global-only chunk-source reads and concat formatting, cached fallback evaluation replacing original repeated expressions, and PutAfter formatting order. Pilot assigned explicit frame redesign and exact regression matrix. ThePropOf case-sensitive number/count formatting is verified source-faithful and needs no change. Live615 unchanged.

- [x] Internal WebGL helper closure reviewed and guarded-integrated from exact hash8c354350b199339529a2c5f02734a21ba4962835a618a24e64bcdbe61d49414d. Mutable authoritative table and Result now reach render_sprite and scene passes; ordinary skips, camera/pass ordering and selected motion validation timing preserved. Frozen combined checkpoint `20260915T142614Z`:603 compiler errors (12 fewer than615), unchanged source/Cargo manifests, zero changed-range primary errors. Browser execution remains unverified. Evidence: `.cache/native-bevy/evidence/webgl-helper-integration/`.
- [ ] Object property-setter full family assigned for proposal to Luna High pilot. Xtra correction expanded to one disjoint session.dispatch_global classifier hunk so deferred operations remain pending rather than queued script errors; evaluator session lifecycle remains separately owned.

Navigator evaluator review3 withheld (eval67aea4c0, sessiona31857f8; snapshots `/private/tmp/eval-root-review3-*`). Root-return and fallback frame redesign improved, but ConvertIndex/ConvertEnd were pushed after Evaluate on the LIFO stack and therefore execute before operands. Actual compile JSON contradicts pilot zero-owned claim: eval.rs2641 partial move and3314 moved name. Requested ordering helper, actual primary-span parsing, fresh frozen hashes and full focused chunk/put/delete/lazy-command fixtures. Live603 unchanged.

- [ ] Object setter family approved in isolation with corrected suspension semantics: original 297da4a awaits cast preload (including handled fetch/parse failure) before Flash invalidation and script continuation. Property loads must not alter preload barriers; canonical driver must preserve Void/Null ignored-value behavior. Evaluator and Xtra shared-file changes remain serial navigator merges.
Navigator resumed review: evaluator stage now contains corrected LIFO conversion ordering and requested chunk/concat/fallback/lazy-parse fixtures; final frozen compile and delta review still pending. Xtra live-map pending validation is present, but cloned Datum argument vectors still require actual owned transport preparation and synchronous routing must avoid preparing then discarding pending work. No new live source integration;603 remains authoritative.

- [x] Owned evaluator three-file batch reviewed and guarded-integrated from review4 snapshots (eval5ee69fa2, sessiona31857f8, drivera5b2cb46): replaces ambient recursive evaluator with owned frames, capability-checked suspension/completion, scope validation and reset cleanup; preserves chunk conversion order, fallback reevaluation, concat and lazy command parsing. Frozen combined checkpoint `20260915T144308Z`:534 compiler errors (69 fewer than603), unchanged full Rust/Cargo manifests, zero owned primary errors. Focused regression fixtures are present but runtime tests remain unexecuted. Evidence: `.cache/native-bevy/evidence/evaluator-integration/`. Remaining callers and owned request execution are assigned for proposal.

Navigator post-evaluator audit: live Rust/Cargo sources still match frozen534 checkpoint. All53 datum dispatcher primary errors occur inside legacy player_call_datum_handler, which retains active call sites in Movie stepFrame, manager, player, score, commands, Havok callbacks, events and construction. Removing it without equivalent owned dispatch would lose behavior; next executor proposal must preserve recursion/depth handling and route those calls. Transport preparation review confirmed Multiuser send checks connectivity before reading arguments and Text consumes only payload; Curl completion resolves current callback after fetch. These ordering constraints were sent to the Xtra pilot for the approved conversion. All three Luna High pilot handles verified running.

- [ ] Approved isolated evaluator request executor (session/driver/canonical datum classifier; narrow evaluator plumbing): evaluator-owned subordinate drivers, prepared-child execution exactly once, retained external pending actions, and checked completion. Paused parent driver PC/token/depth must remain intact; existing start_handler rejects a second driver and start_child_with_policy advances the parent PC, so neither is an unchanged debugger-eval adapter. Production lifecycle and remaining callers follow after this executor.
Navigator typed Xtra transport review withheld acceptance: owned connect/send/config payloads now exist, but Curl provided-Void callback and mode-before-callback-commit semantics need correction. Multiuser Form B must preserve three ordered first-match key lookups and avoid unrelated value reads; optional conversions and recipient-element default fallbacks must distinguish ordinary conversion failure from foreign ownership. Corrections sent as one batch. Live534 unchanged.

Navigator Xtra review2 snapshots frozen under `/private/tmp/xtra-root-review2-*`; reviewed correction deltas across six mapped files (multiuser218, curl120, manager329, player24, datum dispatcher32, handler manager13 lines). Curl mode-before-commit and provided-Void behavior are corrected in this snapshot. Remaining required corrections: normal teardown drain must collect submanager released queues; disconnect/reconnect must retire prior connection identity; Curl constructor must reject foreign dynamic symbols before conversion fallback. Approved narrow static_datum_to_runtime authoritative-symbol-table propagation because canonical getnetmessage still called its removed Symbol::from_str path; preserve unrelated From conversions for later migration. No live source integration;534 remains verified.

Navigator completed initial full Curl helper diff review against original source (setOption/form/getInfo/static conversions preserved). Additional corrections: callback names and Form B keys must retain symbol_value String support; with_xtra_manager_state must restore its taken manager on panic. Preliminary setter-stage review found FlashObject set_prop still invokes ambient ruffle_set_variable_global under the player borrow; requested smallest owned host-request outcome preserving conversion/error behavior and execution only after borrow release. JS owned-props storage was distinguished from that Flash boundary. No acceptance of these drafts yet.

## Updated stop condition

User narrowed this run to a clean compile (zero compiler errors); tests need not pass. Stop once the compiler gate succeeds. Native/runtime/browser acceptance work remains outside this run after that gate. Continue coherent compiler fixes, preserve behavior, and avoid additional regression expansion. Current verified gate uses locked offline native library test compilation with --no-run;534 errors at checkpoint20260915T144308Z. All three pilots notified.

- [x] Nine-file Xtra ownership/preparation batch integrated from frozen-final, excluding testing-only reorder; session classifier merged without overwriting evaluator. Combined151010 exposed one constructor mutability mismatch, corrected to readonly SymbolTable (manager hash4b515cef). Frozen combined checkpoint `20260915T151158Z`:534 errors, unchanged full source/Cargo manifests, zero changed-range primary errors. Existing legacy Xtra/static conversion errors remain; next compiler-closure pass assigned. Tests were not run and are not the updated stop gate. Evidence `.cache/native-bevy/evidence/xtra-integration/`.

- [x] Three direct compiler repairs applied outside pilot ownership: replace obsolete test-only Symbol spur construction with Symbol::empty, remove deleted global symbol initialization from GIF fixture, clone retained motion-track Symbol at consuming call. Frozen combined checkpoint `20260915T151602Z`:531 errors (three fewer than534), unchanged Rust/Cargo manifests. Evidence `.cache/native-bevy/evidence/compiler-leaf-cleanup/`.
Navigator executor handoff review pending corrections: preserve integrated CastLib/Transform3d/Xtra sync arms accidentally dropped by stale overlay; fix inverted parent-scope validity predicate; validate request capability before dispatch mutations; return child completion outcome instead of discarding it. Compiled isolated506 count is not integrated or authoritative.

- [x] Evaluator executor corrections reviewed and four files guarded-integrated: preserved canonical dispatch arms, checked actions before mutation, valid parent scopes accepted, duplicate children rejected, completion outcome retained. Combined compiler verification pending. Evidence `.cache/native-bevy/evidence/evaluator-executor-integration/`.

- [x] Evaluator executor combined checkpoint `20260915T152453Z`:502 errors (29 fewer than531), unchanged source/Cargo manifests.
- [x] Property-list sprite indexing now passes the existing authoritative SymbolTable to sprite_get_prop; next combined compile will verify this one-line caller repair.

- [ ] Setter batch review completed; corrections pending before integration: preserve unrelated completion tickets, route subordinate evaluator completions, preserve preload finalization after retired preload work, and refresh overlay without deleting accepted evaluator/Xtra code. Three existing cast fixture macro diagnostics assigned for direct assertion repair.
- [ ] Xtra legacy compiler closure under review: runtime StaticDatum conversions now require authoritative owner/table context; parser scalar conversion remains separate. Reject symbol-erasing fallbacks and runtime context-error stand-ins. Root prepared nine explicit network-handler conversions for inclusion with manager caller propagation.
- [ ] Production lifecycle and caller migration explicitly approved as a coherent batch: RuntimeSession must own the real player graph, with retained host session handle and short synchronous borrows, then propagate session/player ID through production event/command/movie/lib callers. Do not adapt ambient static-player storage into a temporary session. Pilots continue Luna High; compiler-only stopping gate remains unchanged.

- [x] Seven-file Xtra/network compiler closure reviewed and guarded-integrated: explicit legacy Multiuser/Curl symbol parameters, owner-aware runtime StaticDatum conversion with separate parsed literal conversion, nine explicit network handlers plus dispatcher calls. Excluded stale datum dispatcher and leech fixture creating unrelated SymbolTable; those caller repairs remain assigned. Combined compile pending; isolated486 was on stale dispatcher baseline and is not authoritative. Evidence `.cache/native-bevy/evidence/xtra-compiler-closure-integration/`.

- [x] Frozen combined checkpoint `20260915T154112Z`:457 compiler errors (45 fewer than502;74 fewer than531 at continuation start), unchanged full Rust/Cargo manifests. Network/Xtra/core StaticDatum files have no primary diagnostics; manager retains unrelated legacy caller errors. Runtime tests were not run.

- [x] Ten-file setter family reviewed and guarded-integrated: explicit property dispatch, distinct cast property-load reservations/cache/notifications, delayed Flash invalidation, owned Flash host writes, retained property completion tickets, leaf symbol signatures and existing fixture compiler repairs. Root narrowed retired-preload finalization to the matching owner. Production executor must still route completed property tickets to primary or subordinate driver; no fake completion/error adapter added. Combined verification pending. Evidence `.cache/native-bevy/evidence/setter-family-integration/`.

- [x] Frozen combined checkpoint `20260915T154325Z`:423 errors (34 fewer than457;108 fewer than531 at continuation start), unchanged Rust/Cargo manifests. Canonical session/driver/castlib compile without primary errors. Remaining getter/leaf/legacy setter caller closure assigned to same pilot; full production lifecycle and remaining Xtra families in parallel isolated stages.

## Disk usage during compiler recovery

- [x] Pause pilots before removing redundant build outputs; preserve all source snapshots and review evidence.
- [x] Complete removal of duplicate temporary and project Cargo caches; retain only `/private/tmp/dirplayer-parser-review-build/target` for this compiler-recovery run.
- [x] Create future stages with `mise exec -- python scripts/create-native-stage.py /private/tmp/dirplayer-<batch>`; this copies only VM/SDK source and fixtures, excludes target/node_modules/.cache/.git/pkg, and configures the shared target directory. Never copy the complete repository into a pilot stage.
- [ ] After each accepted batch, remove its temporary generated outputs. Retain source/diff/manifest evidence until the implementation is complete. Check free disk space before a large build and avoid new per-pilot target directories.

Cleanup verified:125 redundant build/dependency cache directories removed across temporary DirPlayer stages, DirPlayer, and childhood-redux parity/playtest caches. Disk free space rose from105GiB to352GiB during cleanup. Current VM/Cargo source hashes still match423-error checkpoint20260915T154325Z; source snapshots and review evidence retained. The source-only staging helper was smoke-checked (10.2MiB) and its scratch stage removed. All three unfinished stages now point to the one retained shared target. Evidence `.cache/native-bevy/evidence/disk-cleanup-20260915/summary.json`.

- [x] MCP/decompiler compiler migration staged in source-only `/private/tmp/dirplayer-mcp-closure-20260915`: explicit owning symbol table, fallible consumed-symbol formatting, existing public JSON schemas retained. Frozen compiler evidence `owned-review-20260915T162422288476Z`:415 total errors, zero primary diagnostics in two owned files, unchanged manifests. Live bases still match423-error checkpoint.
- [ ] Complete independent pilot review and guarded integration of MCP/decompiler; propagate frontend callers in lifecycle batch, then run combined compiler check.

- [ ] Leaf batch review corrections required before integration: restore real rateShift lookup, String/Symbol property-list keys and symbol-to-string conversions; XML element names must use owning-table display rather than Debug. Remove ambient PlayerDatum access in explicit-context methods. Isolated378 is not an accepted checkpoint.
- [ ] Legacy Xtra review corrections required: preserve set_lingo_global operation through actual owner context, reject foreign symbols rather than emit empty names, and use retained test session symbols rather than per-helper SymbolTable instances. No part of this rejected draft integrated.

- [x] MCP/decompiler two-file migration independently approved by Luna High pilot and guarded-integrated from frozen415-error isolated compile. Live before hashes match423-error checkpoint; combined compiler verification follows. Evidence `.cache/native-bevy/evidence/mcp-decompiler-integration/`. Frontend signature propagation remains in lifecycle batch.

- [x] Combined checkpoint `20260915T163053Z`:415 compiler errors, unchanged full Rust/Cargo manifests, zero primary errors in MCP/decompiler files. Eight fewer than423 net:18 original file errors removed,10 frontend caller signature errors exposed in lib.rs and assigned to lifecycle pilot. Shared target only; runtime tests unrun.

- [x] JS Lingo value-conversion helpers prepared and compiler-checked in existing source-only stage: outward names/formatting return Result with actual owning table, inward object keys intern into supplied table; existing variants preserved. Frozen diff/manifests `/private/tmp/dirplayer-js-conversions-reviewed/`. No primary diagnostics in four modified helpers;430 total isolated errors include new caller propagation.
- [ ] Lifecycle pilot to review/merge JS conversion helper hunks alongside actual callback/registry context migration before live integration. Live combined checkpoint remains415.

- [x] Eight legacy Xtra production files reviewed and guarded-integrated, with original Leech test module retained. Required table conversion/recursive foreign-symbol errors and explicit external host callback context preserve real SetGlobal behavior. Also integrated two existing-context synchronous type-handler call fixes (sprite getter and palette conversion). Frozen overlay390 errors with unchanged manifests; combined check follows. Evidence `.cache/native-bevy/evidence/legacy-xtra-integration/`.

- [x] Combined checkpoint `20260915T164747Z`:390 compiler errors, unchanged full Rust/Cargo manifests,25 fewer than415. Rejected Leech test context changes excluded; root two synchronous types fixes independently approved by Luna pilot.
- [x] Six independent leaf files prepared and frozen-checked:353 errors, zero primary diagnostics in owned files, unchanged manifests (`owned-review-20260915T165038397645Z` in root source-only stage). XML helper calls use supplied player; JS error formatting preserves resolved text. Integration awaits final correction review; sound/manager/types changes remain separate pending preservation of key and duplicate-entry semantics.

- [x] Six-file getter/leaf batch reviewed and guarded-integrated: JSObject, PlayerDatum, XML, Flash legacy serialization, script getters and get/set bytecode callers. XML helpers now use the supplied player, with root corrections independently approved by Luna pilot. Frozen353-error overlay; combined verification follows. Sound remains separately held. Evidence `.cache/native-bevy/evidence/leaf-getter-integration/`.

- [x] Combined checkpoint `20260915T165326Z`:353 compiler errors, unchanged source/Cargo manifests,37 fewer than390; six leaf files have no primary diagnostics.
- [x] SoundChannel plus narrow manager/types callers guarded-integrated from immutable345-error overlay. Restored Symbol/String key support, foreign-symbol errors, last-valid duplicate loopCount semantics, per-segment rate/name through advance/loop, and property-list member resolution. Root corrections independently approved by Luna pilot. Runtime audio tests unrun; legacy no-caller methods remain. Evidence `.cache/native-bevy/evidence/sound-leaf-integration/`.

- [x] Combined checkpoint `20260915T165651Z`:345 errors, unchanged full Rust/Cargo manifests,8 fewer than353 and70 fewer than415. Shared build target only;343GiB disk free. SoundChannel has no primary compiler diagnostics.
- [ ] Remaining async compiler work repartitioned: global pilot owns sprite/cast member typed host requests; leaf pilot owns types/script-instance/timeout caller migration; movie pilot retains frontend lifecycle. Movie isolated426-error draft remains unaccepted because legacy wrappers still convert Pending to errors. Root requested actual completion/resumption routing before next lifecycle acceptance.

- [x] Resumed three Luna High pilots after disk cleanup; no removed caches have reappeared,350GiB free. Shared target remains mandatory. Explicit command/event outcome propagation approved for lifecycle pilot; typed child continuation plans approved for types/script-instance/timeout pilot.
- [x] JS interpreter property bridge candidate frozen and compiler-checked in existing source-only root stage:338 isolated errors, unchanged manifests, zero primary errors in interpreter/host_bridge. All property/index/increment routes now receive the runtime bridge; old ambient Director accessors removed. Candidate `/private/tmp/dirplayer-js-property-bridge-reviewed`, evidence `owned-review-20260915T170643711365Z`.
- [ ] Integrate JS property candidate only with real owner-captured PlayerBridge methods (required trait implementation), independent review and combined compiler verification. Live checkpoint remains345 errors; isolated338 is not an accepted live result.

- [x] Seven-hunk synchronous manager correction independently approved by Luna High and guarded-integrated: six bitmap/script-instance/cast-member calls receive existing execution context; Xtra fallback copies name/id before mutable dispatch. Combined checkpoint `20260915T171327Z`:339 compiler errors, unchanged full Rust/Cargo manifests, six fewer than345. Diff whitespace check passed. Evidence `.cache/native-bevy/evidence/manager-sync-integration/`. Runtime tests unrun; shared target only,350GiB free.

- [ ] Async sprite/cast draft review withheld integration: current typed request classifier removes legacy bodies without an executor, discards prepared W3D load payload, and flag parsing swallows checked reference errors. Pilot assigned actual host execution/completion, retained prepared data and error preservation before frozen handoff.
- [ ] Command callback queue draft review requires an actual queue consumer/host executor, explicit captured session/player at loop construction, owner/generation in pending command, and resumed Abort handling. Queue storage alone is insufficient; lifecycle pilot implementing complete caller family. Live339 remains authoritative.

- [x] Prepared source-owned ScoreInitializer parser boundary candidate: parsed movie behavior parameters retain validated expression source, materialize only with explicit player/symbols; production syntax validator shares existing grammar. Frozen parser/eval candidate `/private/tmp/dirplayer-score-initializer-reviewed`, evidence `owned-review-20260915T173922827562Z`:340 isolated errors, zero primary diagnostics in parser/eval, unchanged manifests.
- [ ] Lifecycle pilot to merge ScoreInitializer with player/score.rs parameter field and four application loops, preserving semantic-error warning/skip behavior. Candidate not live until consumers and independent review complete; authoritative live339 unchanged.
- [ ] Sprite/cast pilot paused by repeated Luna capacity failures after its source edits were preserved; other two pilots remain running. Do not restart completed compiler work or recreate stage; retry same requested model after backoff.

- [x] ScoreInitializer parser candidate independently approved by Luna High; awaiting four score consumer loops and runtime parameter representation before integration.
- [x] Shared PendingAction::ticket accessor independently approved and guarded-integrated; covers CastLoad and every HostRequest variant, avoiding incomplete frontend matches. Isolated unchanged compiler snapshot `owned-review-20260915T174524506687Z`:339 errors, zero driver errors; live base fully matched prior339 checkpoint before one-file integration. Evidence `.cache/native-bevy/evidence/action-ticket-integration/`. Combined check follows with caller integration.

- [x] Combined checkpoint `20260915T174832Z` after shared action-ticket accessor:339 errors, unchanged full Rust/Cargo manifests. The reviewed exhaustive accessor is ready for caller completion paths. Sprite/cast Luna pilot resumed after capacity backoff and has fresh source edits; all three pilots active.

- [x] Root froze seven current types/continuation pilot files against live339, preserving accepted manager fixes and ticket accessor. Isolated checkpoint `owned-review-20260915T175430390265Z`:328 errors, unchanged manifests; zero primary errors in driver/session/dispatcher/timeout/script-instance. Snapshot `/private/tmp/dirplayer-types-root-snapshot-20260915`.
- [ ] Before continuation acceptance: resolve two types.rs703 inference errors (infallible empty-value allocation), preserve virtual handler precedence before builtin/system-event no-ops in ScriptInstance prepare_call, and finish real type/timeout child plan application. Twelve legacy script.rs diagnostics remain; pilot received exact compiler evidence. No live integration of this draft;339 remains authoritative.

- [x] Synchronous Sprite context batch independently reviewed by Luna High, requested callback args0/optional args3 preflight applied, guarded-integrated. Same owning player/table now used in Flash sync arms and explicit sprite/member resolver; original async body byte-for-byte preserved. Combined checkpoint `20260915T180854Z`:317 errors, unchanged full Rust/Cargo manifests,22 fewer than339; diff whitespace check passed. Evidence `.cache/native-bevy/evidence/sprite-sync-integration/`.
- [ ] Corrected Child snapshot `/private/tmp/dirplayer-child-handoff-20260915` preserves Global raw arguments vs Object receiver prepend. Additional review requires timeout-ancestor Forget delegation before generic ScriptInstance Forget no-op. Pilot correcting before integration.
- [ ] Lifecycle282-error draft remains unaccepted: pending command APIs have no execution consumer; score initializer helper reselects ambient session/player and returns empty parameters for missing context. Required actual host executor/resumption and caller-supplied materialization context assigned back to pilot. No part of this draft integrated.

- [x] Four-file script-instance Child dispatch batch reviewed and guarded-integrated. Canonical object calls now start owned child continuations; evaluator Global/Object argument policy, virtual-handler precedence and timeout ancestor forget behavior preserved. Combined checkpoint `20260915T181836Z`:314 compiler errors (three fewer than317), unchanged Rust/Cargo manifests, zero owned-file primary errors. Evidence `.cache/native-bevy/evidence/child-dispatch-integration/`. Compiler-only check; shared Cargo target retained.

- [ ] Review/integrate four synchronous Movie preference/navigation methods and four manager caller arms from `/private/tmp/dirplayer-movie-sync-reviewed`. Actual ExecutionContext and checked owning symbols replace ambient lookups; storage/navigation semantics retained. Frozen isolated evidence `owned-review-20260915T182339673289Z`:308 errors vs live314, unchanged manifests. Awaiting independent Luna High review; not live. Constructor/timeout pilot approved to extend canonical GlobalDispatch/evaluator/driver/object continuations to complete actual plan application.

- [x] Four Movie preference/navigation methods and manager dispatch arms independently approved by Luna High and guarded-integrated. Combined checkpoint `20260915T183352Z`:308 compiler errors (six fewer than314), unchanged Rust/Cargo manifests; changed-file diff whitespace check passed. Foreign consumed references reject before conversion; browser preference, target fallback, fragment and deferred navigation behavior preserved. Evidence `.cache/native-bevy/evidence/movie-sync-integration/`. Runtime tests unrun; shared Cargo target only,350GiB free.

- [ ] Sprite/cast executor frozen root review against live308: isolated `owned-review-20260915T183536316937Z` compiles with293 errors, zero owned primary diagnostics, unchanged manifests. Integration withheld for real direct Flash fallback/readiness preservation, Sprite child receiver argument layout and owner-local Havok step callback mode (including collision distinction). Pilot has explicit approval to fix affected force-routing consumers. Live remains308.

- [ ] Constructor/timeout eight-file handoff reviewed against live308 in root source-only stage. Frozen `owned-review-20260915T183916488995Z`:298 errors, unchanged manifests;12 legacy ScriptRef diagnostics remain in owned family. Integration withheld for discarded evaluator prepared args, constructor receiver argument layout, child ext-call/return policy preservation, and ancestor early-return sequencing. Pilot approved to fix these plus complete ScriptRef/new/birth explicit routing. Command executor review additionally found moved action loses completion ticket; lifecycle pilot owns actual executor implementation, not only injection-point API.

- [x] Eight-file Sprite/cast executor batch reviewed and guarded-integrated, including Luna-approved root W3D fidelity corrections. Typed requests retain owner/args/flags; short-borrow host executors preserve Flash readiness/direct-call behavior and import/load failure results. Havok force callback mode is per-player and restored per turn; step and collision callbacks distinguished. Combined checkpoint `20260915T184906Z`:293 errors (15 fewer than308), unchanged Rust/Cargo manifests; changed-file diff whitespace check passed. Evidence `.cache/native-bevy/evidence/sprite-cast-executor-integration/`. Remaining frontend executor must invoke these concrete APIs and drain callback continuations. Runtime tests unrun; sharedtarget only,349GiB free. JS bridge/loader family assigned to global pilot, leaving command/lifecycle ownership with movie pilot.

- [ ] Corrected constructor final2 handoff merged with live293 typed executor changes in root source-only stage. Two merge-only session fixes preserve Sprite defaults and request-owner validation. Frozen `owned-review-20260915T185543060459Z`:283 errors, unchanged manifests, only12 legacy ScriptRef errors in owned family. Review still requires ObjectV4 delivery policy independent of ExtCall-layer ownership, plus completion of ScriptRef/new/birth caller migration. Pilot reviewing root merge diff. Live remains293.

- [ ] Constructor final3 merged over the reviewed root candidate, preserving live293 Sprite/cast executors and two approved session merge fixes. Frozen `owned-review-20260915T190924716759Z`:272 compiler errors, unchanged Rust/Cargo manifests, zero primary errors in eight owned files. Legacy global birth removal withheld pending caller migration. Integration still requires explicit ScriptRef object new/birth/rawnew and own/virtual handler routes after removal of ambient wrappers; pilot assigned correction. Frontend executor and owned JS setup/registration remain active. Shared Cargo target only;348GiB free.

- [x] Eight-file constructor/timeout/ScriptRef batch reviewed, corrected, and guarded-integrated. Explicit child completion retains prepared args, constructor fallback, ancestor sequencing, ExtCall ownership and ObjectV4 result delivery. ScriptRef own/virtual/static precedence and dispatcher depth cleanup preserved. Combined checkpoint `20260915T192248Z`:272 errors (21 fewer than293), unchanged full Rust/Cargo manifests, zero primary diagnostics in eight owned files. Evidence `.cache/native-bevy/evidence/constructor-timeout-integration/`. Legacy frontend global birth caller remains for the production caller migration. Runtime tests unrun; shared target only.
- [ ] Fixture migration transferred to leaf pilot: testing.rs, testing_shared/mod.rs and Leech cfg(test) require harness-owned session/player/owner and actual symbol table. Partial ambient/fresh-table fixture shortcuts remain unaccepted in movie stage. Other pilots continue production executor and session-owned JavaScript setup/registry integration.

- [ ] Production caller review rejected the candidate-only `retained_session_handle()` / `PLAYER_SESSION_HANDLE` thread-local authority in movie stage. Live272 remains unchanged. Production pilot must thread frontend-captured session/player/owner through callers; fixture pilot separately owns harness context. Manager async wrappers still contain Call/Do/navigation callers and require actual canonical execution before removal.

- [ ] Remaining caller reviews: score initialization needs an executed deferred-completion pump plus AwaitingNew/Reapply continuation phases; helper definitions alone are not completion wiring. JS pilot now owns cast registration and FrameSetup/driver Setup producers in addition to loader APIs. Harness handoff requires real owned async load/frame/eval entrypoints; static-expression eval and mixed global/owned players are unaccepted. Leech production dispatch and local-session fixtures isolated as next independently reviewable batch. Live checkpoint remains272.

- [x] Three-file Leech ownership batch reviewed and guarded-integrated. Static and instance dispatch receive actual player/symbol context; consumed refs validate before mutation. Local-session Leech fixtures preserve assertions without global TestPlayer dependency. Combined checkpoint `20260915T195659Z`:263 compiler errors (9 fewer than272), unchanged Rust/Cargo manifests; changed-file whitespace check passed. Existing manager async-family errors remain outside the narrow instance-dispatch hunk. Evidence `.cache/native-bevy/evidence/leech-owned-integration/`. Runtime tests unrun; shared target only.

Navigator review after cleanup verification: live checkpoint remains263 errors. Harness v2 withheld pending explicit channel/session teardown, preservation of browser scheduling yields, and owned frame advancement. Command pump review requires enqueue wakeup while idle, cancellation of the active command future without detached-task leaks, and execution of reachable CastLoad actions. JS setup resume must select its explicit continuation phase even when the child return policy is absent. Corrections assigned to existing Luna High owners; no candidate integrated and no runtime tests run. Verified all125 removed cache directories remain absent;424GiB free.

Navigator integration review: JS producer handoff received with explicit Setup resume phase; source-only merge onto live263 assigned to preserve newer constructor continuations. Registration drain requires handle/cast identity rather than mutable CastLib borrow across JS invocation; pre-invocation setup scope/ticket validation requested. Harness retirement/installation correction approved with real owner frame scheduling verification. Command future now directly polled; requeue wake suppression and caller cancellation completion remain under correction. No live source integration yet.

- [x] Three-file harness migration reviewed and integrated: explicit runtime owner, real command queue, owned load/init/eval/frame callsites, short context borrows, retirement closes sender and removes player; browser teardown preserves RAF yields and explicit frame stepping. Combined unchanged checkpoint20260915T202030Z reports265 compiler errors. Six harness diagnostics are required production API definitions/signature still assigned to movie pilot (five owned entrypoints plus command loop). This is an intermediate migration, not a clean compile. Evidence `.cache/native-bevy/evidence/harness-owned-integration/`; no runtime tests run.

Frontend ownership proposal approved: Luna pilot w3d_events_design owns lib.rs/js_api.rs instance BrowserPlayerHandle, authoritative symbols, captured owner callbacks and per-instance notification coalescing; production pilot retains movie/mod/events/score/commands/manager scope. Required browser consumers and renderer routing must be coordinated explicitly, without ambient wrappers. CastLoad transport reviewed in staged production code; driver-registered requests still require the same real fetch/apply path. Live compiler checkpoint remains265.

JS rebase review identified remaining merge obligations: use full explicit interpreter bridge snapshot, fix moved child policy, and preserve Ancestor/Completion return policies through SetupCallback resumption (constructor fallback, timeout completion, remaining ancestors, delivery, ExtCall release). Setup pre-invocation ticket/owner/scope validation now implemented in stage. Production pilot has added module-level owned parsed-movie loader and continues actual evaluator/lifecycle entrypoints. No further live integration;265-error checkpoint remains authoritative.

- [x] Ten-file JS ownership integration verified: session-owned registry, weak bridge handles, explicit DirectorRef conversions, queued cast registration outside VM borrow, ticket/owner/scope-checked Setup execution and constructor/ancestor/timeout completion policies. Combined frozen checkpoint20260915T204235Z:245 compiler errors,20 fewer than265, unchanged full manifests; root stage had zero primary errors in eight core JS/driver/session files. Required production activation drain/Setup consumer wiring remains assigned. Single trailing-space cleanup recorded separately after check; no semantic change or runtime tests. Evidence `.cache/native-bevy/evidence/js-owned-integration/`.

Production loader/evaluator/timeout handoff review withheld integration: proposed eval_lingo_command_owned is synchronous EvalId/EvalTurn while approved harness API awaits DatumRef; preserve low-level start helper separately and implement complete owned async wrapper. Timeout target dispatch drops child variants via wildcard, and global branch retains RefMut across await. Pilot correcting all variants and sequential continuation through actual executor before acceptance. Live245 remains authoritative.

Current review dependencies after live245: built-in Call/SendSprite/AllSprites batch must preserve static fallback, last non-Void aggregation, non-Abort error continuation, dynamic lookup and JS child policies. Frontend consumer migration approved for provider/UI/network/Flash/MCP; rejected module-global browserHandle replacement, requiring per-provider ownership/disposal. Reset must preserve loaded movie while cancelling only its owner resources. Evaluator wrapper must execute real child/external actions through EvalId/EvalAction completion, with no never-resuming pending future or dropped timeout child variants. All three Luna pilots active; source-only stages/shared target remain required.

- [x] Post-cleanup revalidation: all125 recorded removed cache directories remain absent;420GiB free. Three Luna High pilots resumed with source-only stages and one shared Cargo target.
- [ ] Owner freeze review: top-level evaluator external queue has no executor caller; taking pending requests removes the registry entry required by submission. Require actual request execution with retained inflight capability/sender before accepting batch. Events owner coordinator migration approved in parallel with this correction.
- [ ] Evaluator broadcast continuation must retain full BroadcastPlan including lazy static fallback, passed/error policy and last non-Void aggregation; pilot correction approved. Browser Flash callback/frame captured-handle migration approved, preserving frozen review handoffs. Live compiler checkpoint remains245; no staged count is treated as live acceptance.

- [ ] Frozen browser review requires cancellation guard after awaited font initialization, instance-qualified callback registration/disposal, and owner-qualified network events. Current netLoader listeners process every taskId-only window event and have no disposer; per-provider captured handles alone do not isolate delivery. Corrections sent to frontend pilot alongside pending per-owner notification coalescing/reset integration.

- [ ] Static fallback review: preserve events.rs same-member/handler/arguments recursion guard across suspended driver/evaluator callbacks, release on terminal/cancel paths, and stop fallback on event_stopped as well as !passed. Current candidate lacks active_static_event_handlers integration; correction coordinated between global and events pilots. Evaluator inflight route registry now implemented in working stage; concrete external request executor remains required before acceptance.

- [x] Independent combined source-only preview: ten frozen runtime/global/frontend Rust files merged without textual conflicts. Locked offline lib test-no-run compiled138 errors with unchanged full source manifests and zero direct session/driver/commands/player-score diagnostics. Evidence `/private/tmp/dirplayer-mcp-closure-20260915/.cache/native-bevy/evidence/owned-review-20260915T233926694208Z`; compact diagnostics sent to pilots. Preview remains unaccepted pending reviewed runtime corrections; authoritative live baseline245. Confirmed missing validate_lingo_expr_syntax helper dependency in frozen score parser. Shared target only;421GiB free.

- [ ] Combined-preview follow-up: movie pilot must bind detached evaluator route before reborrowing session (if-let RefMut lifetime), deliver already-terminal EvalTurn directly rather than resume consumed capability again, and implement real remaining external actions. Renderer family approved with owner-bound actual SymbolTable/backend and coordinated frame/movie callers. Flash snapshot has callback disposer but singleton vmCallbacks/window bridge remain explicit uncompleted ownership work.

- [x] Four-file global broadcast batch reviewed and integrated live: canonical Call/SendSprite/SendAllSprites dispatch, complete driver/evaluator BroadcastPlan metadata, lazy handler/static fallback resolution, implicit receiver policy, last non-Void aggregation, non-Abort continuation, owner/registration-qualified static recursion guards, and typed import preparation. Source matches frozen pilot gate234748176784Z; outside-owned sources matched live before integration. Live locked offline lib test-no-run gate `.cache/native-bevy/evidence/owned-review-20260915T234923093578Z`:245 errors, full manifests unchanged; whitespace check passed. Integration evidence `.cache/native-bevy/evidence/global-broadcast-integration/`. Runtime tests unrun; remaining legacy manager caller migration assigned to global pilot.

- [ ] Remaining caller batch coordination: global pilot owns manager legacy cleanup (isolated229 errors, pending sole mod caller) and handlers/movie.rs family; movie pilot owns mod/events/score/commands/session lifecycle and real async executor. Events must preserve ScopeResult including passed/error across Pending/Waiting and cancellation. Frontend owns renderer and consumer migration; renderer disposal now detaches owned listeners, but canvas container must be a captured React element rather than a global DOM selector. Shared target/source-only stage policy remains in force.

- [x] Refreshed14-file source-only integration preview compiled108 errors (previous preview138), unchanged manifests; shared target only. Evidence `/private/tmp/dirplayer-mcp-closure-20260915/.cache/native-bevy/evidence/owned-review-20260916T022728387242Z`. Core errors are missing movie executor(2) and request Clone(1); remaining families events47/mod28/lib22/rendering3 plus narrow callers/fixtures. This is not live acceptance; live remains245. Scheduler review requires persistent in-flight action futures through cooperative polls and idle/active command paths; temporary per-iteration futures would drop suspended work. Provider/network follow-up snapshot received for review.

- [x] Refreshed combined source-only preview gate `owned-review-20260916T024600951932Z`:130 errors, exit101, unchanged full Rust/Cargo manifests. This includes newly introduced movie executor and scheduler drafts; it is not live acceptance (live245). Frontend lib diagnostics fell from22 to2; owned draft fixes remain commands8/events47/movie27/mod37 plus renderer/harness/narrow callers. Exact diagnostics and refresh provenance retained in `/private/tmp/dirplayer-mcp-closure-20260915/`. Shared target reused;421GiB free.
- [x] Focused JavaScript check of frozen lib-family callback bridge passed: overlapping sprite7 events routed to distinct owners; unknown/disposed owner callbacks dropped; disposing one registration preserved the sibling. This verifies Flash callback routing only, not broader browser parity.
- [ ] Review corrections assigned: scheduler detaches one request at a time and retains cancellation metadata; movie Go resolves labels after new-movie mount and preserves in-handler lifecycle; renderer/Flash operations carry owner; frontend font preference and Xtra plugin registries must use owned handles. All Luna High pilots resumed after cleanup.

- [x] Independently observed pilot check14 complete (`/private/tmp/eval-current-check14.log`):150 isolated-stage errors, no primary errors in commands/session/driver. Counts are not comparable to combined130 because manager/frontend dependencies are stale in this stage. Reviewed one-request scheduling and retained in-flight cancellation metadata; requested stable queue extraction and reset cleanup of evaluator/static-event state. Full caller family still events43/mod27/harness2; no runtime tests or new live integration.

- [x] Integrated cumulative browser foundation:27 files plus minimal player module ownership hunks, before/after hashes guarded and original files retained. Live locked/offline lib-test no-run `owned-review-20260916T030015213567Z`:212 compiler errors (33 fewer than245), unchangedfullmanifests. Owned browser handle, provider/UI/MCP routing, renderer state, networkowner filtering and basic mouse/key entrypoints are now in live tree. JS syntax, integrated-file whitespace and live Flash owner/disposal routing check passed. Evidence `.cache/native-bevy/evidence/frontend-cumulative-integration/`.
- [ ] Frontend acceptance remains incomplete: field/IME and nested input, session reset/renderer rebinding, owned Xtra lifecycle, remaining global callbacks/exports. Two Xtra free-export arity mismatches inadvertently included in cumulative snapshot assigned for immediate correction. Core movie/event/scheduler migrations remain staged; no native runtime tests executed.

- [x] Applied reviewed two-function frontend signature correction with before/after hashes: legacy register_external_xtra(name) and complete_external_xtra_load(name, success) now match current external module until full owner-aware migration lands. Evidence `frontend-cumulative-integration/arity-correction.json`; last full live compile remains212, not a claimed new count.
- [x] Refreshed combined source-only preview from livefrontend plus current core/movie/manager candidates: `owned-review-20260916T030615904288Z`97 errors, unchangedfullmanifests; no primary diagnostics in commands/session/driver/eval/score. Remaining events41/mod31/movie14/frontend/rendering/harness. Three missing owner_key_string diagnostics came from omitted frontend dependency hunks in preview mod; three-way merged those after the check, preserving staged conditional cleanup. No lower count claimed without recompile.
- [ ] Xtra WIP review rejected newly introduced static mut OWNER_REGISTRY: string-qualified process globals do not satisfy independent session/thread ownership. Pilot to use actual player/session-owned ExternalXtraState and owner-validated browser handle completion/registration. Input/renderer/reset work remains active.

- [x] Combined source-only preview gate `owned-review-20260916T034121730162Z`:22 errors (previous97), exit101, unchanged full Rust/Cargo manifests. No direct driver/evaluator/session/commands/score diagnostics. Compact errors retained in preview evidence; remaining frontend dependencies, legacy init callers, event loader argument, and movie test assertion assigned to their pilots. Runtime tests not executed; live last verified212.
- [ ] Frontend frozen handoff review rejected stale whole session/manager files that would remove accepted broadcast/static-guard behavior; pilot rebasing minimal renderer/Flash changes. Unsupported external::retire_owner dependency and reversed selection endpoint bug also assigned.
- [ ] Lifecycle review requires faithful phase ordering and complete startup/system timeout/streamStatus/stepFrame/prepareFrame behavior, plus owner validation before pending-init and post-await cleanup mutations. Full ScopeResult child callback support must preserve suspended parent drivers. Compiler cleanliness alone does not accept these routines.

- [x] Combined preview `owned-review-20260916T035524534168Z`:10 compiler errors, exit101, unchanged full Rust/Cargo manifests, no runtime tests executed. Includes current full-ScopeResult callback core and rebased frontend with minimal weak renderer binding; manager merge preserved approved legacy async removal. Remaining errors:7 legacy init/W3D callers in mod.rs,3 legacy renderer calls missing owning symbols. Last verified live212 remains distinct.
- [ ] Event review corrected cumulative handled/static fallback semantics and owner-scoped stop restoration; post-await owner validation and actual callback-wrapper migration remain staged. Frontend Flash bridge review found Rust(sprite,owner) versus JS(owner,sprite) argument mismatch plus ambient queue fallback; pilot fixing with isolation fixture before acceptance.

- [x] Combined 17-file refresh gate `owned-review-20260916T040636014726Z`:3 compiler errors, exit101, unchanged full source/Cargo manifests. Legacy W3D initialization callers now compile in combined tree; event callback wrapper and new event fixtures also compile. Remaining:one mod frame-end draw caller, two owned RAF references to accidentally removed request_animation_frame utility. No runtime tests executed and no new live integration; live last verified212.
- [ ] W3D fidelity follow-up: preserve per-owner elapsed animation/particle clocks and static-only frame/movie callback hierarchy (Static target is an optional argument, not a member identity). Renderer/Flash owner queue fixture and complete lifecycle phase preservation remain required before acceptance.

- [x] Combined preview `owned-review-20260916T041403754149Z`: locked/offline native lib test compilation succeeded with zero errors and unchanged source manifests. This is preview evidence, not live integration or full-plan completion.
- [ ] Native runtime acceptance: the 352-test serial run timed out after 300 seconds at the owned global-event fixture, with six earlier failures. Isolated bounded diagnostic sweep underway; cast cancellation/driver, events/foreign references, and grammar failures assigned to the existing Luna High pilots. Preserve assertions and rerun the full suite after fixes.
- [x] Disk discipline rechecked: all 125 removed cache directories remain absent and 420 GiB free; pilots paused during audit, now resuming source-only work with one shared Cargo target.

- [x] Bounded isolated diagnostic sweep of the exact zero-error test executable (SHA256 `1a99187db698bd20f6060a93b964c88dae267cf275d7c1a4b7a0fa90c5eb1551`): 352 tests, 337 passed, 11 failed, 4 timed out at 15 seconds each. Evidence: preview gate `runtime-tests/isolated-sweep/summary.json` and per-failure logs. Isolation is diagnostic only; full same-process acceptance still pending. All failures assigned across existing pilots, with no additional build caches.

- [x] First runtime correction batch verified in combined preview gate `owned-review-20260916T043329690012Z`: zero compiler errors with unchanged manifests including grammar. All four targeted formerly failing tests pass; related evaluator (19), property-list (8), and list-handler (11) suites pass, 38 total. Evidence `focused-runtime.json` and `related-runtime.json`. Reviewed allocator guard/cast and score fixtures/driver completion policy/string fixture corrections are staged for next combined gate; event pump/W3D traversal corrections still under review.

- [x] Guarded live integration of the four verified parser/property files: exact live-before hashes matched reviewed preview baselines, backups and hashes retained at `.cache/native-bevy/evidence/runtime-parser-property-integration`. Preview related tests: 38 passed. Full integrated live compile/runtime gate remains pending the broader core batch.

- [x] Second runtime batch compiled with zero errors and unchanged source manifests: preview gate `owned-review-20260916T044026698077Z`. Serial suite excluding the separately pending benchmark finished in 0.46s: 350 passed, 3 failed, 1 filtered. Prior event and score hangs resolved. Remaining failures: setter policy assertion after Ret, missing Invalid opcode diagnostic name in W3D malformed-handler test, and handler-gap test failing in the full serial suite. Full benchmark workload migrated to explicit sessions in next preview; not yet compiled or run.

- [ ] Lifecycle review of owned initialization still finds missing startupDo/startupGo consumption, pending streamStatus dispatch, prepareMovie/startMovie/exitFrame timeout dispatch, and faithful frame/sprite/filmloop beginSprite routing plus begin_sprite_called bookkeeping. Split prepare/start/enter phases alone do not meet legacy behavior. Owner-validated cleanup of in-frame/prepare/enter guards must cover errors and cancellation, and begin_all_sprites must validate captured owner after awaited phases. These remain implementation requirements after runtime suite repair.

- [x] Third combined preview gate `owned-review-20260916T044949503305Z`: zero compiler errors, unchanged manifests; full serial native suite now 354 passed, 2 failed, 0 skipped, no timeouts (1.14s reported by harness). Actual SetObjProp cast-loading completion, benchmark workload, W3D target argument, invalid-opcode diagnostics, and prior cancellation/ownership regressions pass. Remaining failures are static parser/evaluator `not true` handling and the handler-gap fixture using legacy globals with an owned harness; corrections active. Evidence `runtime-command.json`, `runtime-summary.json`, and runtime logs.

- [ ] Fourth preview gate `owned-review-20260916T051341192417Z` compiled the owned input-wait/cancellation and recursive static-prefix changes without diagnostics, but failed on one newly registered callback fixture: `expect_err` requires `ScopeResult: Debug`. Pilot correcting the assertion with an explicit match; no production Debug change required. Callback preservation/error/reset tests are newly registered and remain unexecuted.

- [x] Fifth native preview gate `owned-review-20260916T051541768967Z`: zero compiler errors, unchanged manifests. Full serial runtime: 358 passed, 3 failed, 0 skipped/no hangs. Earlier static-not and input-gap failures now pass including reset-during-wait cancellation. All failures are newly registered callback fixtures expecting a real pending action before driving the initial Waiting ticks; corrections assigned. WebAssembly library check started on frozen preview using the shared target, results pending.

- [x] Sixth native preview gate `owned-review-20260916T052505025100Z`: zero compiler errors, unchanged source manifests; full serial suite 360 passed, 1 failed, 0 skipped, no hangs (1.04s). Real waiting-turn callback fixtures now verify primary retention/pass and reset rejection. Remaining non-Abort callback error preservation test receives Abort; pilot tracing the cause without weakening the assertion.
- [ ] Browser preservation: frozen-preview wasm32 library check exited101 with15 compiler errors and unchanged source manifests. Diagnostics retained in `/private/tmp/dirplayer-wasm-preview-check.jsonl`; explicit symbol-table bridge/decompiler arguments, owned browser loading/reset, and handler-name serialization assigned as a coherent batch to existing pilots. Native-only success does not prove browser compatibility.

- [x] Reviewed minimal browser compiler batch staged in combined preview (`wasm-fix-refresh-1.json`): owned URL loader wrapper, owner-key browser reset, checked error-pause handler spelling with terminal fallback, and removal of unused ambient player_trigger_error_pause. Full browser verification waits for coherent JS snapshot/decompiler symbol plumbing and callers. No whole-file overwrite of older pilot frontend state.
- [ ] Remaining callback failure traced to ScriptDriver::fail_current_frame leaving child frames live while RuntimeSession::finish_eval_child checks parent stack validity. Pilot approved to unwind the child with token checks before validating parent and preserve original non-Abort error; regression must also prove parent resumability. Correction not yet accepted or tested.
- [ ] Xtra review found static-probe response losing selected plugin identity, insufficient owner checks around reentrant host calls, and numeric load IDs able to collide across owner states. Pilot correcting typed responses and exact-owner capability storage, including synchronous load-completion receiver retention; draft remains outside verified preview.

- [x] Seventh native preview gate `owned-review-20260916T053438898412Z`: zero compiler errors, unchanged manifests, full serial suite 361 passed/0 failed/0 skipped (0.99s). Child error frames now unwind before parent validation, preserving original errors. Guarded integration of20 reviewed Rust files and5 reviewed frontend files into live checkout; all live-before guards matched and backups retained at `.cache/native-bevy/evidence/native-green-integration-20260916T053739Z`. Live full compile/runtime gate begins next; wasm/browser preservation and full Stage2 ownership remain incomplete.

- [x] Live native gate `owned-review-20260916T053740336966Z`: locked/offline library-test build succeeded with zero compiler errors and unchanged manifests. Exact executable passed all361 tests serially (0.60s) and with default parallel runner (0.46s), zero ignored/filtered/failures. Full Rust/Cargo/grammar source matches the green preview. This supersedes historical live212-error results; browser-target checks, complete runtime ownership removal, native backend/fidelity/performance acceptance remain open.

- [ ] Browser handoff review rejected stale lib.rs that removed BrowserPlayerHandle and notification code that rediscovered retained_session_handle/active_player_id. Pilot must rebase onto current verified source, carry explicit owner context, and propagate SymbolTable through snapshot/debug/reset callers as one coherent batch. No rejected browser draft integrated into live; live native361/361 serial and parallel remains authoritative.
- [ ] Xtra APIs now have a reviewed direction for selected-plugin response identity, owner-bound exact-once load capabilities and preserved synchronous-completion receivers. Pilot implementing actual driver/session/command/handler and nested HostOp routing; standalone unused APIs do not satisfy ownership migration.

- [x] Refreshed live lexical ownership inventory after native integration:671 legacy-accessor references,62 legacy-task findings,17 static-mut declarations,45 mutable-singleton candidates,16 thread-local candidates, plus JS registry findings. Evidence `.cache/native-bevy/evidence/ownership-inventory-20260916`. These are lexical review candidates, not semantic proof or a completion percentage; Stage2 removal gate remains unmet.

- [x] Browser compiler repair preview: initial wasm15 errors reduced to3 in diagnostic check2, then zero in manifest-checked gate `wasm-review-20260916T055404050055Z` (locked/offline wasm32 library check exit0, unchanged manifests). Includes explicit snapshot/decompiler symbol plumbing and removal of obsolete free inspector exports; owner-context error debug propagation and legacy access removal remain incomplete. Native regression build for same batch running; live remains prior361-test green baseline until reviewed integration. Browser runtime preservation is not yet verified.

- [x] Browser repair batch native regression gate `owned-review-20260916T055503180136Z`: zero compiler errors, unchanged manifests; all361 tests pass with default parallel runner (0.79s), including exact non-Abort child error and suspended-primary resumption. Source manifest identical to zero-error wasm gate `wasm-review-20260916T055404050055Z`. Guarded8-file live integration with backups at `.cache/native-bevy/evidence/browser-compiler-integration-20260916T055805Z`; full live Rust/Cargo/grammar hashes equal tested preview. Browser runtime and remaining error-notification/ownership migration acceptance still open.

- [ ] Expanded wasm `--tests` gate `wasm-tests-review-20260916T055950475989Z`:15 errors in native-only fixtures importing native TestPlayer or using async_std::block_on with wasm return type (). Production library remains zero-error. Native-harness cfg corrections assigned without weakening native assertions; future verifier now includes integration-test source hashes.
- [ ] Browser runtime readiness: local Chromium1217/Node/wasm tools/dependencies present, but runner hardcodes vm-rust/target and must honor shared target + exact Cargo artifact. dpt_shapes movie absent in public; reference snapshot submodule uninitialized, and current e2e returns no-reference success, which is not snapshot comparison evidence. Independent two-BrowserPlayerHandle state/reset/dispose fixture planned without licensed assets; no baseline promotion authorized.
- [ ] Independent pilot reviews rejected Xtra late same-name load completion/duplicate attach/direct synchronous host bypasses and lifecycle missing frame semantics/stale-owner mutation/phase cleanup/host-clock sampling. Corrections active; Xtra pilot owns one serialized shared-cache native no-run diagnostic build cycle before returning the full error batch. Drafts remain unintegrated.

- [x] Reviewed and integrated native-only test fixture cfg corrections for cursor/handler-gap, owned event pump, callback pump, and only the GIF movie-change harness test; pure GIF control tests remain wasm-enabled. Evidence: `.cache/native-bevy/evidence/wasm-fixture-cfg-integration-20260916T061310Z`.
- [x] Reran full wasm test compilation after fixture cfg integration; see `wasm-tests-review-20260916T061429422734Z` below. Native coverage remains361 passing tests.

- [x] Full wasm test-target compile gate `wasm-tests-review-20260916T061429422734Z`: locked/offline check succeeds, zero errors, unchanged Rust source/test/Cargo/grammar manifests. This is compilation only, not browser runtime acceptance.
- [x] Integrated reviewed browser runner shared-target configuration, exact Cargo artifact selection, locked build, bounded diagnostic buffer, output-directory deletion guard, and matching server paths; both scripts pass Node syntax checks. Evidence: `.cache/native-bevy/evidence/browser-runner-integration-20260916T061540Z`. Runtime browser isolation fixture remains in progress.

- [x] Post-cfg native regression gate `owned-review-20260916T061554482785Z`: locked/offline library-test build succeeds with unchanged manifests; test result: ok. 361 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.32s Exact executable hash and runtime logs retained.

- [x] Integrated asset-independent browser two-handle fixture with real stage-size reads, overlapping player IDs, reset generation changes, first-handle drop, and surviving-handle reset/mutation. Runtime validation pending; evidence `.cache/native-bevy/evidence/browser-isolation-integration-20260916T063359Z`.
- [ ] Execute browser handle isolation on freshly built wasm and dedicated server with bounded timeout and retained-on-failure video only.

- [ ] Real browser isolation gate active: evidence `browser-isolation-runtime-20260916T063614Z`, one shared-cache release build and dedicated port9237, no server reuse,60s Playwright timeout; do not infer pass from build progress.
- [ ] Browser reset review found direct player reset bypassing session-owned cancellation. Reviewed adapter correction staged; integrate only reset hunk and net owner refresh after frozen browser gate completes, preserving live cancel_eval_continuations.
- [ ] Xtra fallback review found unclaimed probes must preserve async builtin/child dispatch and lazy argument encoding. Current draft fallback subset is rejected pending complete continuation routing and regression tests.

- [x] Browser gate `browser-isolation-runtime-20260916T063614Z` completed: release wasm compiled successfully (8m30s), frozen source unchanged; selected real fixture started but timed out60s. Bounded JS diagnostic using identical generated wasm proves both constructors and stage mutations succeed; first reset throws from outdated test bridge `onFlashResetAll` calling handle-required Flash teardown without a handle. Owner-key bridge correction pending; runtime acceptance remains open. Built wasm/glue hashes retained.

- [x] Real Chromium isolation gate `browser-isolation-runtime-20260916T065419Z` passes: exactly `test_browser_player_handle_isolation` ran,1 passed/0 failed (3.1s Playwright), unchanged source manifests and built artifact hashes retained. Owner-key Flash teardown stub fix resolves prior reset exception. This proves asset-independent handle state/reset/drop isolation, not Flash-content fidelity or pending-work cancellation.
- [x] Added build-script `rerun-if-changed=build.rs` to prevent unrelated browser-template edits retriggering Rust release compilation. Next Rust gate will verify the directive; it does not suppress Cargo tracking Rust source changes.

- [x] Integrated production Flash reset callback owner-key forwarding in bridge/types/frontend to avoid wasm handle reentry during mutable reset. Focused Node production-bridge routing/disposed-owner isolation check and callback syntax check pass. Evidence `production-flash-reset-20260916T070449Z`; full production browser reset rerun pending canonical reset/Scene3d integration.

- [x] Integrated reviewed player-owned Scene3dStore, exact-owner renderer cache invalidation/resource pruning, and canonical BrowserPlayerHandle reset adapter with net-owner refresh and owner-scoped Flash teardown. Guarded backups/hashes: `.cache/native-bevy/evidence/scene-reset-integration-20260916T070834Z`.
- [ ] Compile native/wasm and run isolation/regression tests for Scene3d/reset batch; source integration alone is not acceptance. Shared compiler slot currently assigned to lifecycle diagnostic.

- [x] Scene3d/reset native gate `owned-review-20260916T070930207602Z`: zero compiler errors with unchanged manifests; exact executable passes364 tests/0 failures/0 ignored (0.47s), including new owner-local scene reset/cache identity regressions. Full wasm test-target check underway separately; live browser rerun after canonical reset still pending.

- [x] Scene3d/reset full wasm test-target gate `wasm-tests-review-20260916T071153628392Z`: zero errors, successful locked/offline check with unchanged manifests. This is compile evidence; regenerated-browser runtime and real Flash lifecycle coverage remain pending.

- [x] Refreshed lexical ownership inventory after Scene3d/reset integration: `.cache/native-bevy/evidence/ownership-after-scene-reset-20260916T071803Z`. Global Scene3d store/accessors removed; remaining findings still require semantic migration/review, not a completion percentage.

- [x] Disk cleanup reverified: all 125 removed cache directories remain absent; current free space 415 GiB. Build stages remain source-only and all pilots use `/private/tmp/dirplayer-parser-review-build/target`.
- [ ] Lifecycle draft gate4 compiled, but full runtime had 369 passes and two failures. Fixture corrections are staged for gate5; filmloop review additionally requires behavior-cache invalidation and redraw after actual frame changes. Not integrated or accepted.
- [ ] Xtra evaluator review found `execute_eval_external_global` calls the host while the command pump holds `session.borrow_mut()`. Require an owned external request returned to the pump, host execution after releasing the borrow, and owner/action-checked completion before acceptance.
- [ ] Flash host draft rejected for introducing a new process-global `flashOwnerHosts` map and borrowing VM state before queueing callback arguments. Require frontend/harness-owned host closures and raw owned callback payloads converted at the command boundary. Not integrated or accepted.

- [x] Independently inspected lifecycle staged gate6: zero compiler errors and unchanged source/manifests; full runtime completed in 0.47s with 370 passed and one failed. Evidence: `/private/tmp/dirplayer-eval-callers-current-20260915/.cache/native-bevy/evidence/owned-review-lifecycle-gate6-20260916T` and sibling `owned-review-lifecycle-runtime-gate6-20260916T`. Remaining suspended-frame fixture panics on a name-table index; corrections require consistent cast/script names and proper argument bytecode, preserving the real pending/cancellation assertion. This is staged evidence, not live acceptance.
- [ ] Xtra borrow-boundary revision now returns an owned external turn to the command pump; verify synchronous host reentry and reset-during-host completion before its first compile gate. Flash closure-owned host/raw-command correction remains active. Shared Cargo cache remains serialized.

- [x] Independently ran exact lifecycle gate7 executable with SHA recorded at `owned-review-lifecycle-gate7-20260916T/root-runtime/command.json`: 370 passed, one failed, 0.32s. The first real suspension and same-owner drop flag restoration pass; failure is the second invocation failing to suspend. `invoke_script_callback_owned` retains a pending evaluator child while awaiting its receiver without a drop cleanup guard; exact-child cancellation and subsequent usability are under investigation. No fixture assertion weakened, no live integration.
- [ ] Xtra staged host boundary now supports a scoped injected host for borrow/reset regression tests. Review requires action validation before the first host side effect, post-host validation before declined-probe fallback, and evaluator cleanup on errors. Shared build slot reassigned from idle lifecycle work to Xtra after process inspection verified no active compiler/test.
- [ ] Flash closure-owned revision removes the added global host map and queues raw callbacks through generation-owned senders. Syntax checks are insufficient; focused lifecycle, overlap, late-registration, and teardown-reentry behavioral tests are required before compilation/integration.

- [x] Inspected Xtra staged raw build log `/private/tmp/xtra-build3.log` and focused log `/private/tmp/xtra-tests.log`: compile succeeds; six evaluator fixtures pass. Exact-executable full serial suite at `/private/tmp/evaluator-external-turn-20260916/evidence/native-suite.json` fails with 381 passed/3 failed (V4 call arguments and two load receiver retention cases). Entire Xtra batch remains unaccepted; fixes and full native/wasm gates required.
- [ ] Preserve canonical BrowserPlayerHandle reset, net-owner refresh, and owner-key Flash reset while merging Xtra stage; review found stale baseline reversions. Add post-host validation to nested Xtra dispatch and assert stale completion cannot resolve a new waiter before valid completion.
- [ ] Lifecycle callback cancellation revision is source-frozen at `/private/tmp/dirplayer-eval-callers-current-20260915/lifecycle-cancellation-freeze-20260916T/manifest.json`; no new ambient cancellation task, explicit callback completer invariant, and filmloop cache/redraw/one-shot fixture included. Await compile/runtime verification after Xtra releases shared cache.

- [x] Root verified frozen lifecycle/cancellation gate `owned-review-20260916T092729000730Z`: locked/offline native compile has zero errors and unchanged full manifests; exact executable SHA retained. Full default-parallel suite: 371 passed/1 failed, 0.32s. Filmloop cache/redraw/one-shot regression passes; same-owner second-frame suspension still fails.
- [ ] Root source trace found owned static-event dispatch enters a reentry guard, awaits a handler, then releases it only after await. Dropping the event future leaves the guard active and can suppress the next prepareFrame. Pilot assigned owner-checked RAII across all owned static-event paths, alongside exact-ticket pending-completion cleanup. Full cancellation acceptance remains open.

- [x] Lifecycle static-guard gate `owned-review-20260916T094840448017Z`: zero compile errors, unchanged manifests; exact executable inventory checked before execution; 373 passed/0 failed/0 ignored in 0.29s. This proves same-owner repeat dispatch after dropped frame, replacement isolation, filmloop regression, and exact-ticket late-completion regression in the staged lifecycle batch.
- [x] Corrected Xtra handoff `/private/tmp/xtra-owned-full-20260916/manifest.json`: raw native fullsuite384 passed/0 failed and wasm test-target build success inspected. Live base and staged owned-file hashes match handoff. Browser runtime remains unverified.
- [x] Created source-only combined lifecycle/Xtra/Flash preview `/private/tmp/dirplayer-combined-reviewed-20260916` (10.6 MiB initial source; shared target only). Three-way merge preserved reviewed current base; resolved Flash callback type conflict in favor of owner-key signatures and preserved production reset key forwarding. Combined Flash lifecycle harness independently passes4 tests, but logs missing-capability initFlashBridge call; correction required before browser acceptance.
- [ ] Combined native gate `owned-review-20260916T100807399689Z`: unchanged manifests, three compiler errors in new Flash capability/raw command handling. Pilot assigned all three plus missing-capability bridge setup directly in combined preview; no live source integration yet.

- [x] Cleanup audit: 124 unique paths from current deletion logs remain absent; `df` reports 408 GiB free. Source-only stages and one shared Cargo target remain required. No new build cache created.
- [ ] Combined Flash source review rejected production bridge acceptance: `initFlashBridge` still overwrites owner-qualified PlayOwned with a single-owner closure despite five passing harness tests. Require actual production A/B registration, dispatch, and B-disposal regression coverage. Ordered raw JSON conversion and targeted behavior propagation also require independent semantic review.
- [ ] Combined Xtra follow-up owns session/eval/driver/commands: execute object/load external requests and declined-probe asynchronous builtins, and cancel exact evaluator children on owner reset/removal. Earlier 384-test handoff did not cover these missing execution paths; combined compile/runtime acceptance remains open.

- [x] Root reran staged Flash lifecycle harness after production-route test addition: six passed. Review still rejects closure-chain routing and out-of-order teardown retention; explicit owner producers and lifetime-owned routing correction active.
- [x] Combined native compile `owned-review-20260916T103027814923Z` completed with unchanged full manifests and three errors: lib.rs callback mutable-context mismatch plus E0500/E0501 from nested session borrow during async-global lowering. Full diagnostic set assigned together; no runtime tests executed.
- [ ] Frozen Xtra continuation revision adds ExternalXtraLoad execution, object host routing, typed movie requests, and owner child retirement. New classification fixtures are not end-to-end completion evidence. Local Multiuser/Curl pending branch and updateStage host clock remain explicit gaps. Targeted event revision restores all-behavior delivery, stopEvent semantics, and missing-sprite handling; new regressions await compiler gate.

- [x] Combined native gate `owned-review-20260916T103219010559Z`: zero compiler errors, unchanged full manifests. Exact compiler-artifact executable SHA and full logs retained; runtime 397 passed, 2 failed, 0 ignored in 0.30 s. Failing ExtCall/TellCall fixtures expect old untyped request shapes; semantic assertions must remain when updated.
- [x] Same frozen batch wasm full test-target check `wasm-tests-review-20260916T104840960208Z`: eight diagnostics representing four unique defects, unchanged manifests. Owner symbol display and browser harness moved senders/exported capability conversion assigned together. No browser runtime acceptance.
- [ ] Flash production callback chaining replaced with existing owner callback routing, seven harness tests reported. Root found main-world bridge host still ignores ownerKey sent by updated client; actual producer migration and roundtrip regression required.

- [x] Combined native gate `owned-review-20260916T115334467254Z`: zero compile errors, unchanged manifests, exact executable 399 passed/0 failed/0 ignored in 0.35 s.
- [x] Combined wasm full test-target gate `wasm-tests-review-20260916T121057941786Z`: zero compiler errors, unchanged manifests after explicit ForeignSymbol-to-InvalidReference correction in wasm-only breakpoint reporting.
- [x] Root independently ran seven Flash lifecycle tests and one main-world bridge owner-routing roundtrip test successfully. These are mocked browser transport tests, not actual Ruffle content/browser runtime acceptance. Ruffle AVM1 LocalConnection still lacks owner identity and is rejected with multiple owners; producer migration remains open.
- [ ] Integration inventory at combined `integration-evidence/current-inventory.json` identifies 27 changed/new source/tooling files. Every captured existing source baseline matches current live checkout. Raw JSON duplicate stored-path fix still pending as isolated patch; live source integration and fresh browser runtime remain pending.

- [x] Broadened native verification from library-only to all native test targets. Migrated 26 obsolete symbol-API/step_native calls across tests/physx.rs and tests/lingo/command_parsing.rs without weakening assertions or adding ignores. Final `native-tests-review-20260916T122047868879Z` compiles every target with zero errors and unchanged source manifests.
- [x] Exact native test artifacts: library 401 passed; integration target 99 passed/1 failed/2 ignored; PhysX 15 existing ignores; native web target has zero tests. The single failure is missing public/dcr_sulake/habbo_v7/habbo.dcr, also verified absent in live checkout. Full native suite is not claimed green. Logs/hashes retained under the all-target gate.
- [x] Final combined wasm full-test gate `wasm-tests-review-20260916T122155644287Z` passes with unchanged source manifests. Raw JSON duplicate stored-path regressions are included in native401.
- [x] Guarded integration of 29 reviewed files into live checkout, with every existing baseline hash checked, source-only compressed rollback archive, and final hashes verified: `.cache/native-bevy/evidence/combined-integration-20260916T122307Z`. Integrated JS tests:8 passed/0 failed.
- [ ] Fresh real Chromium two-BrowserPlayerHandle gate active at `browser-isolation-runtime-20260916T122339Z`; shared Cargo target only. No full browser/content parity acceptance yet.
- [ ] Local Multiuser/Curl typed executor migration approved in existing source-only preview, based on `integration-evidence/post-live-baseline.json` matching395 live files. Live source remains frozen for browser verification.

- [x] Live browser gate `browser-isolation-runtime-20260916T122339Z` compiled successfully but failed after60s with unchanged full source manifest. Reused-artifact page-error diagnosis found module initialization failure: browser stub lacks `disposeExternalXtraHost` re-export. Exact generated import audit:24 names, exactly one missing. Pilot assigned real production re-export and early pageerror reporting; browser runtime remains unaccepted.
- [ ] Next preview batch includes local-Xtra owner-configured socket callbacks and queued Targeted event ownership; root review flagged ambient allocator reclamation after targeted await and incomplete suspended-reset coverage. Live source remains isolated from preview edits.

- [x] Disk discipline: source-only staging via scripts/create-native-stage.py; all compiler gates share /private/tmp/dirplayer-parser-review-build/target. Preserve pending source patches and concise evidence; remove obsolete build/dependency/browser copies after review.
- [x] September 16 residual-cache cleanup completed: removed and verified 55 obsolete compiler/dependency/browser directories (29.42 GiB logical size), preserving source, test fixtures, active preview, and shared Cargo target. Exact deletion manifests: /private/tmp/dirplayer-residual-cache-cleanup-results-20260916.jsonl and /private/tmp/dirplayer-browser-cache-cleanup-results-20260916.jsonl.

- [x] Real Chromium browser handle isolation: browser-isolation-runtime-20260916T123506Z passed 1 test with unchanged manifests after repairing the missing production disposeExternalXtraHost template export. This verifies browser handle isolation, not Flash content fidelity.
- [x] Production wasm package build: production-wasm-20260916T123637Z exit 0, unchanged source manifests, generated VM bindings refreshed.
- [ ] Production frontend build: production-frontend-20260916 exit 1, unchanged source manifest. VMProvider INIT_OK optional browserHandle violates reducer state; Luna pilot assigned complete TypeScript diagnostic batch.
- [ ] Combined next event/Xtra preview native all-test compile: native-tests-review-20260916T124659647821Z has 10 diagnostics, unchanged manifests; pilot corrections assigned. Review also requires typed child continuation execution and moving Multiuser socket host calls outside mutable session borrows.
- [x] Isolated Ruffle owner-key source patch host harness: pinned Bun executed 9 tests, all passed after supplying existing browser template fixture. Actual rebuilt Ruffle wasm producer remains unverified; extension test must exercise the new owned producer with two active owners.

- [x] Six-file production frontend repair accepted: required INIT_OK owner handle/reset callback, owner-bound inspector/network initialization, typed debug payload/line arrays, narrow Flash capability types, snapshot map cleanup. Pinned tsc --noEmit zero diagnostics; production npm run build exit 0; root independently ran production Flash lifecycle/bridge harness 8 passed, 0 failed. Evidence production-frontend-20260916/{tsc.log,build.log,flash-tests.log,accepted-source.json}. Accepted frontend sources mirrored into the existing preview without copying artifacts or caches.

- [x] Reviewed Ruffle owner-bridge source integrated (seven files) against clean submodule 79d1ca0f45d79e28c3a0658bbc6b8430d26ef308 with baseline hashes and compressed source rollback. Core wasm build, wasm-bindgen generation, and typed TypeScript builder method all pass; selfhosted webpack bundle passes. Shared CARGO_TARGET_DIR is honored by the build helper; generated artifact hashes recorded in ruffle-owner-build-20260916. Dependent frontend/host changes remain staged pending actual AVM1 Chromium producer verification; native core compile is active.

- [x] Ruffle native core check also passes: mise exec -- cargo check --manifest-path ruffle/Cargo.toml -p ruffle_core --locked --offline, shared target, native-summary.json exit 0. Source changes remain seven reviewed files; actual AVM1 Chromium owner-producer runtime gate is being implemented.
- [ ] Event-loop review: explicit loop now receives owner/session, but acceptance requires rechecking play/skip after waits and a second session executing a marker handler, not merely remaining idle. Nested/main startup callsite migration remains pending. Xtra review requires prepared socket-only host work, rejected resources dropped outside session borrow, owned send-pump lifetime, and real child-to-host-to-parent execution tests.

- [x] Actual Chromium AVM1 owner producer gate passed: two real players loaded ruffle/tests/tests/swfs/avm1/localconnection/test.swf, each produced channel/test/[] with its exact owner; after A removal B reload produced further sends, no late A SWF traffic or page errors. Producer capture endpoint is test instrumentation; production host routing is covered separately. Artifact/SWF hashes: ruffle-owner-build-20260916/avm1-owner-browser.json. Four dependent host/test source files integrated with baseline hash checks; public Ruffle bundle matches tested generated artifacts. Integrated live host harness and production frontend build both pass.
- [ ] Frozen event/Xtra native test-target gate native-tests-review-20260916T145733541540Z:15 diagnostics; wasm-tests-review-20260916T145823593796Z:6 diagnostics, both unchanged manifests. Root families: event helper return/import, new loop startup callers, non-Send child test spawn and ScopeResult Debug assertion. Pilots assigned complete family repairs; no runtime tests claimed for this preview.

- [x] Combined native and wasm all-test compiler gates pass with zero errors and unchanged manifests: native-tests-review-20260916T150326258682Z and wasm-tests-review-20260916T150606522144Z. Reviewed startup caller integration removes the proven dead standalone allocation and passes captured owners; full nested ownership remains open.
- [ ] Exact native library binary ran 412 tests: 410 completed pass, one child test failed on native js_sys::Date::now, and the receiver-loop test hung until the 120-second process timeout. Focused backtrace identifies handlers/movie.rs::execute_nothing_owned; cross-platform wall-clock fix and bounded continuously-driven receiver harness fix are staged for recheck. Integration target unchanged:99 pass,1 failure from absent Habbo DCR,2 existing ignores; physics target0 failures,15 existing ignores. Runtime artifact hashes/logs preserved under the native gate.
- [ ] Xtra resource review still requires a production consumer of take_teardown_requests; moving socket resources into teardown records alone does not drain them on reset/close. No live integration accepted on compiler success alone.

- [x] Disk cleanup follow-up: verified 165 previously removed caches remain absent; removed three obsolete export-MTL targets and shared debug incremental cache (13.03 GiB logical size), preserving source and executable evidence. Manifest: `/private/tmp/dirplayer-extra-cache-cleanup-results-20260916.jsonl`.
- [ ] Ongoing disk discipline: one shared Cargo target; serialize builds; use `CARGO_INCREMENTAL=0` for repeated disposable-stage verification; source-only stages; remove obsolete artifacts after acceptance and check disk usage before large builds.

- [x] Cleanup final verification: another 12 obsolete parity dependency directories removed (7.11 GiB), preserving newest dependency set and all sources/artifacts. Total follow-up removal20.14 GiB; disk453 GiB free. `/private/tmp/childhood-obsolete-dependency-cleanup-20260916.jsonl`.
- [x] Combined native/wasm compiler gates zero errors, unchanged full source manifests: `native-tests-review-20260916T152658281031Z`, `wasm-tests-review-20260916T152924157436Z`; shared cache, incremental disabled. Exact library binary completed411 passed/1 failed in2.03s, no overflow or process timeout.
- [ ] Actual MovieAsync child callback test times out after discarding returned continuation turns. Pilot correcting fixture to use the real production owner scheduler; callback ScopeResult and ticket retirement remain required. Xtra teardown source review requires moving active resources before player removal and an injected resource-drop probe through production drain paths.

- [x] Root cause resolved: evaluator-child completion returning cooperative Waiting retained started=true, so the production scheduler could not select it again. Child Waiting now clears started exactly as the primary driver does. Native all-test compiler gate `native-tests-review-20260916T153706904807Z` has zero errors/unchanged source; exact library binary412 passed/0 failed/0 ignored in0.33s on the default stack. The real MovieAsync child callback and two-session receiver loop both pass. Staged batch, pending teardown review and wasm recheck before live integration.

- [x] The412-pass source batch also passes wasm all-test compiler gate `wasm-tests-review-20260916T153947796213Z`, zero errors and unchanged source. Shared Cargo cache9.9 GiB, disk452 GiB free after verification.
- [ ] Browser preview/subscription source review found new initial score/channel snapshot helpers still use the global JS callback slot. Owner-key routing and actual two-owner callback-delivery tests required before acceptance; checking subscription flags alone is insufficient.

- [x] Reviewed teardown and frontend source batch integrated into existing combined preview with verified seven-file teardown baselines, frontend patch check, source-only rollback archive and before/after hashes: `integration-evidence/teardown-frontend-20260916`. Native all-test compiler gate `native-tests-review-20260916T155501621864Z` zero errors/unchanged source; exact library415 passed/0 failed/0 ignored in0.34s. Wasm all-test gate `wasm-tests-review-20260916T155718913118Z` also zero errors/unchanged source.
- [x] Root independently executed pinned Node owner-scoped callback-routing test: two owner registrations and retired-owner rejection pass. This is JS routing evidence, not actual Rust-to-browser producer verification.
- [ ] Browser follow-up must guard selected-channel snapshot for an empty/unloaded score (current get_channel_snapshot indexes directly), verify real handle callback routing/reentry/reset, and exercise real WebSocket teardown with surviving second owner.
- [ ] Nested source review held from integration: activation currently includes non-Movie sprite members, recursive async frames need bounded indirection, startup cancellation/owner-validated rollback and actual browser input routing need correction together.

- [x] September 16 callback/evaluator review: five source files integrated into the existing combined preview with checked baseline/final hashes and a small rollback archive (`integration-evidence/callback-eval-20260916T161655Z`). Adds synchronous evaluator Xtra teardown collection, its production-path regression, empty/out-of-range channel snapshot guard, and real Rust-handle callback routing/reentry fixture.
- [ ] Actual browser gate for that batch: `browser-isolation-runtime-20260916T161659Z`; results pending. Compiler or direct JS routing success does not establish browser callback behavior.
- [ ] Remaining reviewed batches: real WebSocket two-owner reset/late-event fixture, nested lifecycle/input ownership, and ongoing score/channel notification ownership. Nested helper availability alone does not prove production input is routed.
- [x] Cleanup recheck: 191 recorded removed cache paths remain absent; regenerated shared incremental directory is empty. Disk452 GiB free; shared Cargo target9.9 GiB; current DirPlayer checkout3.1 GiB. Pilots use existing source-only stages and no independent Cargo targets.

- [x] Actual Chromium handle callback boundary passed (`browser-isolation-runtime-20260916T161659Z`, 1 passed/0 failed in1.4s, unchanged manifests): Rust handle operations route through production owner registrations, callback reentry, reset/rebind and second-owner survival. A missing source-stage snapshot-report script caused a post-report warning after the passing test; the script was copied from live for the next run.
- [x] Expanded native all-test compile `native-tests-review-20260916T162038294346Z`: zero errors and unchanged sources. Exact library artifact416 passed/0 failed/0 ignored in0.31s, including synchronous evaluator Xtra-close collection. This remains a reviewed preview batch pending live integration.
- [x] Expanded wasm all-test gate `wasm-tests-review-20260916T162339181948Z`: zero errors/unchanged sources after correcting the new fixture command result from DatumRef to unit. Earlier failed wasm gate `20260916T162217450052Z` is retained as diagnostic evidence.
- [ ] Combined actual browser handle/WebSocket lifecycle run active: `browser-isolation-runtime-20260916T162406Z`. Native tests and compilation alone do not establish real socket cancellation or surviving incoming delivery.
- [x] Source-stage creation now writes Cargo build.incremental=false and prints CARGO_INCREMENTAL=0; navigator independently checked Python syntax and TOML config. No generated stage/cache was created for this check.

- [x] Real combined browser run `browser-isolation-runtime-20260916T162406Z` finished with unchanged sources: handle test passed again; socket lifecycle test failed on the deliberately injected stale-generation event. It emits `InvalidReference: stale Multiuser socket event` as a current-player script error. This is an ownership/lifecycle bug; the batch is not browser-accepted or integrated live. Pilot correcting stale-event discard semantics and adding native coverage; genuine current socket errors must remain observable.
- [x] Source-stage report/font omissions repaired by copying four existing support files (10,691 bytes), with hashes in `integration-evidence/browser-support-source-20260916.json`. No additional dependency/build cache created.
- [ ] Notification producer patch rejected in review because it added ambient spawn_player_local and removed Score notifications without replacement producers. Pilot must deliver the approved session-owned outbox with actual producers/drain and tests before integration.

- [x] Fresh broader native artifacts for the416-test batch retain99 integration passes,1 absent-Habbo-DCR failure and2 existing ignores; PhysX0 failures/15 existing ignores. Live and playtest fixture paths were rechecked absent. No fixture assertion was weakened. Evidence remains under `native-tests-review-20260916T162038294346Z/runtime-*`.
- [x] Stale Multiuser socket correction reviewed and integrated into the shared preview: missing instances, stale generations and foreign/dead owners are discarded before current-instance mutation or script error reporting. Valid socket errors and callbacks remain delivered. Exact two-file manifest and rollback: `integration-evidence/stale-socket-20260916T163739Z`.
- [x] All native test targets compile0 errors/unchanged manifests (`native-tests-review-20260916T163744984938Z`); exact library418 passed/0 failed/0 ignored in0.31s. Added production command-path and manager regressions validate unchanged current state and callback queues on stale/foreign events.
- [x] Wasm all-test compiler gate `wasm-tests-review-20260916T163913566337Z` passes0 errors/unchanged manifests.
- [ ] Actual browser lifecycle rerun active at `browser-isolation-runtime-20260916T164003Z`. Live integration has31 candidate files with all existing baseline hashes verified; remains pending browser acceptance.
- [ ] Nested review caught parent-space coordinates retained in child commands and incomplete synthetic fixture setup; pilot correcting. Notification review caught JS dispatch still under the outer RuntimeSession RefMut; pilot moving dispatch to an external post-borrow drain. Neither unaccepted patch is in the shared preview or live checkout.

- [x] Real browser gate `browser-isolation-runtime-20260916T164003Z` passed both handle isolation and Multiuser socket lifecycle tests (2 passed/0 failed, Playwright1.2s), unchanged manifests. Valid B incoming delivery survives A reset; stale socket events cause no current script error. Report generation also completed.
- [x] Integrated31 reviewed files into live with checked before/after hashes and source-only rollback: `.cache/native-bevy/evidence/owner-lifecycle-integration-20260916T164332Z`. Copied exact compiler/runtime/browser evidence, without binaries or caches. Pending nested/notification work was excluded.
- [x] Live production gate `.cache/native-bevy/evidence/production-owner-batch-20260916T164337Z`: wasm package, TypeScript and frontend build all exit0. Initial owner-test invocations used the wrong runtime/combined window mocks; isolated pinned runners pass8 Flash tests,1 bridge test and two-owner VM callback assertions, with production source unchanged (`isolated-tests-summary.json`).
- [x] Reviewed nested source batch integrated into existing combined preview only (`integration-evidence/nested-reviewed-20260916T164413Z`). A moving-baseline patch reversion was caught and corrected immediately; accepted socket logic/tests remain intact (`preserve-accepted-socket.patch`, `merged-commands.json`). Live source was unaffected.
- [x] Nested batch native all-test gate `native-tests-review-20260916T165001128477Z` passes0 errors/unchanged source after fixing5 unique compiler defects together. Exact library425 passed/0 failed/0 ignored in0.29s. Tests cover session registry, stale numeric-id replacement, child-local pointer payload and keyboard state. Wasm gate pending; no native nested-content fidelity claim.
- [ ] Notification outbox review now requires native queue consumption and bounded, guarded reentrant dispatch; no unaccepted notification code integrated.

- [x] Disk cleanup follow-up: removed 27,860 SHA-256-identical temporary evidence copies (15.12 GiB logical) while retaining originals in the live checkout, plus three unused childhood-redux playtest Cargo targets (4.37 GiB). Verified all removed paths absent and retained evidence present; disk470 GiB free. Manifests: `/private/tmp/dirplayer-duplicate-evidence-cleanup-20260916.jsonl`, `/private/tmp/childhood-unused-target-cleanup-20260916.jsonl`. Active source stages preserved; obsolete polling shell stopped. DirPlayer mise now sets `CARGO_INCREMENTAL=0`; shared target and source-only staging remain required.

- [x] Reviewed/applied staged real FileIO reset wiring and native late-completion regression, plus SysMenu exact-owner checks/parser correction. Native `native-tests-review-20260916T190226481764Z` and WASM `wasm-tests-review-20260916T190400789323Z` both reach type checking with one remaining MessageBox string ownership error (reported twice for library/test targets), unchanged source manifests. Runtime acceptance pending correction; live source not yet updated.
- [ ] Host sink review corrections: preserve undelivered events ahead of reentrant new events, enforce terminal overflow consistently, and verify actual browser lifecycle reentry. Playback review corrections: register public play fixture, replace inverted immediate-replay assertion with epoch/handler effects, and report owned loop errors. Existing Luna High pilots active; no additional build stages.

- [x] Reset/SysMenu eight-file integration accepted at `.cache/native-bevy/evidence/sysmenu-reset-integration-20260916T191307Z`: native all-test compilation zero errors; exact library binary441 passed/0 failed/0 ignored; WASM all-test compilation zero errors; actual Chromium5 fixtures passed including reset during SysMenu alert. All source manifests unchanged and integration hashes checked. Real session reset now swaps NetManager task state; SysMenu state is player-owned with typed detached host effects. Production gate active `production-owner-batch-20260916T191333Z`. Updated408-file live baseline retained in combined integration evidence.

- [ ] Next approved extension family: BudAPI atomic process state and ambient helpers, OpenURL ambient host call, and Curl compatibility wrappers migrate together to explicit owner state/typed detached effects; native two-owner/reset/stale-reference and real browser reentry verification required. JS Lingo TLS runtime/object registry removal remains a separate pending family. Flash review still rejects pre-instance global loading fallback and stale same-sprite completion clearing replacement state; actual capability browser test required before acceptance.

- [x] Live production gate `production-owner-batch-20260916T191333Z` passed all six steps: WASM release/package, TypeScript, frontend build, Flash lifecycle, bridge routing and VM owner callbacks. Full production source manifest unchanged; all eight integrated reset/SysMenu hashes independently reverified. No outstanding failure in this accepted batch. Broader ownership/native plan remains active.

- [ ] Direct-host lifecycle and corrected bounded-mailbox patches merged into existing combined review stage, preserving LIVE408; twelve-file checkpoint stored as `integration-evidence/direct-host-compiler-checkpoint.json`, live unchanged. Native `native-tests-review-20260916T192146509923Z` unchanged,12 diagnostics across missing OwnerKey import, test module paths and Result-returning stage setter closures. Direct Node host routing test passes. Pilot correcting full compiler family plus actual exported-handle callback reentry, clear-during-callback tail ordering, callback teardown lifetime and missing-sink retry wake loop before acceptance.

- [x] Direct-host WASM diagnostic pass `wasm-tests-review-20260916T192438339443Z` confirms same12 diagnostics/three families as native, unchanged source; no additional browser-specific compiler family. All408 accepted live source hashes reverified unchanged. Node direct host routing passes; runtime acceptance remains pending full compiler/reentry/retry correction.

- [ ] Host tail/retry corrections staged with all accepted e2e module registrations preserved. Pilot now owns only combined-stage lib.rs/session.rs/host_events.rs compiler repairs and the sole shared Cargo slot for native+WASM all-test checks and exact library runtime tests. Root pauses combined writes/Cargo during this assignment and reviews resulting before/after hashes before integration. Actual exported JS-handle lifecycle test remains required; current Rust-direct fixture cannot establish wasm-bindgen reentry behavior.

- [ ] BudAPI/OpenURL/Curl frozen family review underway. Required corrections: replace clipboard spawn_player_local with explicit owner-bound completion, validate against explicit manager/player authority instead of SysMenu owner, preserve optional-argument defaults without swallowing foreign references, and return explicit native unsupported results for unimplemented host effects. Combined-stage compiler ownership remains assigned exclusively to host pilot; no root builds active.

- [x] Direct-host staged compiler family resolved: native `native-tests-review-20260916T193652219773Z` and WASM `wasm-tests-review-20260916T193854872702Z` both zero errors with unchanged and identical full source manifests, independently checked by navigator. Pilot retains Cargo slot until exact native runtime test handoff. Actual browser tail/reentry and live integration remain pending.

- [x] Navigator reverified all408 accepted live source hashes and identical native/WASM compiler manifests for the staged direct-host batch. Compiler correction patch reviewed; live remains unchanged.
- [ ] Added actual generated JavaScript handle lifecycle reentry and native retained-work overflow regression to the combined stage. Browser gate `browser-isolation-runtime-20260916T194657Z` running with shared Cargo cache and incremental disabled. Native runtime447-pass report is a manual terminal capture; raw artifact rerun still required after added regression.
- [ ] Review found exported mutable handle methods still synchronously invoke lifecycle callbacks despite releasing the session borrow. Pilot proposing detached boundary covering set/clear/reset/drop. BudAPI fallback also uses shared localStorage despite claiming owner-local state; owner-storage correction required before integration.

- [x] Actual generated-JS regression confirmed exported receiver aliasing: browser gate `browser-isolation-runtime-20260916T194657Z` exit1, unchanged sources, generated lifecycle callback failed with recursive-use/unsafe-aliasing error; cleanup then raised ownership-while-borrowed and aborted remaining fixtures (0 passed/2 failures including init). No browser acceptance claimed. Approved detached bounded lifecycle delivery covering bind/clear/reset/drop, including no-loop shutdown and mutable callback reentry; pilot implementing in existing source stage.
- [ ] Native all-test compile rerun started for retained-owner-work overflow regression; exact library runtime will be captured to raw logs. Clipboard fallback is now player-owned in candidate source; missing-browser-clipboard capability/throw path is under review.

- [x] Native all-test compiler gate `native-tests-review-20260916T195208975939Z`: zero errors, unchanged sources. Exact library artifact runtime captured under `runtime-vm_rust`:448 passed/0 failed/0 ignored in0.34s, unchanged executable hash. Includes retained-owner-work overflow cancellation regression. Host batch remains staged pending browser lifecycle correction; actual generated JS failure above remains unresolved.
- [ ] BudAPI family patch dry-run caught malformed doubled-newline diff formatting; pilot regenerating relative-path patches from retained original bases before integration. Flash routing review additionally requires surviving-player routes after latest installer disposal and distinct queued frame-setting semantics.

- [x] Broader exact native artifact now has raw logs at `native-tests-review-20260916T195208975939Z/runtime-mod`:99 passed/1 failed/2 ignored, unchanged executable. Failure is missing `public/dcr_sulake/habbo_v7/habbo.dcr`; inspected fixture configuration specifies only that local path and no acquisition URL. Existing assertion retained unchanged.

- [ ] BudAPI patch `19558a25862b2eca34a6a4c7dd8c2470897e1afde5a1b8efe7cd225e91041308` rejected during independent diff review: session hunks revert corrected host_events module paths and delete accepted native overflow cancellation regression. No integration performed. Pilot directed to retain only the37-line dispatch addition and audit unrelated reversions; commands/driver/browser fixture/module hunks inspected as additions only. Playback pilot retains exclusive combined integration/Cargo ownership.

- [x] Navigator ran exact source-stage Bun Flash lifecycle file:7 passed/5 failed, unchanged frozen source verified (`/private/tmp/dirplayer-flash-owner-routing-review-exact-20260916.json`). New ticket completion, pre-instance queue and queued goto/frame-setting tests pass; existing bridge paths fail on missing localConnectionSendOwned/registerFlashOwner in older stage support. Pilot must rerun against preserved combined bridge source. This is diagnostic evidence, not batch acceptance. Use explicit `./scripts/...` Bun paths to avoid matching retained snapshot test copies.

- [x] Navigator verified BudAPI session correction contains only intended37-line dispatch addition. Prepared corrected ten-file family `/private/tmp/dirplayer-budapi-reviewed-session-fixed-20260916.patch` SHA256 `ce8f8fc75bc69449725f1d931537a073b13528de8c1c136c6a50d5a2f7c9794a`; movie pilot authorized to integrate alongside playback before its compiler pass if not already running.
- [ ] Detached lifecycle first freeze `dad536f50c59052f3966a95b1ff96b0f6d74f693eae5ce2787f412ee0d49dae4` reviewed, four before/after file hashes verified. Required fixes: repeated reset must preserve queued retirement obligations, callback failure must not strand finite unattempted tail, and overflow must not leave partially queued cleanup unscheduled. Pilot adding regressions; patch not integrated.

- [ ] Playback pilot integration is materially underway: nine Rust source files differ from the last448-test combined gate (datum, JS API, Flash/3D/sprite handlers, manager, player loop, score, WebGL). Combined ownership and sole Cargo slot remain with movie pilot; these moving sources are not yet verified.
- [ ] Approved JS-Lingo ownership family after read-only audit: remove runtime/object/current-runtime TLS and ownerless APIs, route JS-object method/property effects through typed explicit-owner requests, use checked non-wrapping object allocation, and move Math.random to runtime state with explicit seed parameter. Native/browser two-owner, reset/reentry, exhaustion and RNG isolation tests required. Xtra pilot works existing isolated source stage; preserves frozen BudAPI and movie Flash interfaces.

- [x] Detached lifecycle inflight-reservation correction reviewed: queued+active capacity accounting, attempted/stale reservation release and reserved tail restoration. Patch `85e699df4fbccfd7d83a82098703bf02292e41bfa04b9647f4d743fd9cce9b9c` cleared for combined compiler checkpoint, offered movie pilot alongside corrected BudAPI. Preserve current combined compiler fixes, playback edits and sink-rebind PumpPending wake absent from older lifecycle base.
- [ ] Lifecycle full-queue Drop retirement guarantee still requires actual generated browser test; global pilot checking no-loop/full-capacity cleanup. Flash raw event/LocalConnection capabilities confirmed enqueue without session borrow; async Lingo callback still needs runtime verification. Shared numeric pinTarget map remains a concrete two-owner seek interference risk for follow-up.

- [ ] Combined integration advanced to actual lifecycle inflight queue code plus BudAPI/public playback browser fixtures (navigator inspected current source). Movie pilot directed to finish current playback/Flash/BudAPI/lifecycle compiler checkpoint before adding deferred pin-state or callback follow-ups. No new compiler gate exists yet; last verified library result remains448 tests from pre-integration source.

- [x] Terminal retirement reserve delta reviewed and hashes verified: `/private/tmp/dirplayer-detached-terminal-reserve-20260916.patch` SHA `7101ea8898e2df7ab152fd9350062cfb81c2ba2db9e37768b4a9e28a1f098310`, applies after lifecycle85e699. Adds bounded one-slot terminal allowance and matching tail capacity plus actual generated full-queue free regression; offered integration pilot before compiler checkpoint. Runtime acceptance pending.
- [ ] Integration pilot paused/resumed once to request explicit interim status and compiler-first checkpoint after repeated absent handoffs. Current source preserved. Read-only process inspection found no waiting patch/Python/Cargo/mise subprocess; no new compiler result claimed.

- [x] Navigator paused combined writer and ran full diagnostics on unchanged source: native `native-tests-review-20260916T202032213575Z`52 diagnostics/29 source locations; WASM `wasm-tests-review-20260916T202115847386Z`71 diagnostics with13 additional locations. Deduplicated rendered errors `/private/tmp/dirplayer-combined-compiler-families-20260916.json`. No runtime acceptance.
- [ ] Movie pilot resumed with exclusive combined writes/Cargo and concrete full-family repair: restore accepted host mailbox/producers omitted from mod.rs integration, complete missing session playback and stop/restart/net-transition/SoundManager helpers, fix Xtra return type, repair WASM fixture type/arity/missing helper. Preserve all playback additions; use retained before source for restoration, no whole-old-file overwrite. Follow-up scope deferred until native/WASM compile and exact raw library tests.

- [x] Additional preservation audit verified every saved integration-before hash; compared Rust function definitions across retained files. Only removed definitions are four known host-mailbox helpers in mod.rs (`/private/tmp/dirplayer-integration-function-preservation-review-20260916.json`); behavior still requires tests.
- [ ] Reassigned concrete combined compiler repair to fresh Luna High pilot `combined_compiler_repair` after pausing previous movie integrator. Current source preserved, no rebase restart. New pilot owns combined writes/sole Cargo slot and complete42-location native/WASM repair list; notification and JS-Lingo pilots remain isolated.

- [ ] JS-Lingo first frozen batch `2a8001a54e7f30bc7df95fb1163342922678a2d54ea7374c45a8499798fb3cf0` rejected for completion gaps: method `this` bound to runtime global instead of receiver, get_prop_explicit unused while script property-read wrapper remains fail-closed, no explicit seed constructor, missing required owner/reset/reentry/browser object regressions. Pilot correcting entire family in isolated stage. No combined integration performed.

- [x] Fresh compiler pilot native gate `native-tests-review-20260916T203032879876Z`: errors reduced52→8, unchanged source. Four failing call sites reference three undefined owner-scoped stop/restart/net-transition helpers. Navigator verified those definitions are absent from old movie stage as well; pilot authorized explicit-owner implementations preserving legacy lifecycle and resource semantics. Remaining WASM/runtime gates pending.

- [x] Disk cleanup recheck: paused all DirPlayer pilots and confirmed no active DirPlayer compiler; removed 2,936,856,576 allocated bytes of shared native/WASM incremental cache. Sources and verification evidence retained. Manifest: `/private/tmp/dirplayer-incremental-cleanup-20260916-final.json`. Keep one shared target, disable incremental compilation on every build, and remove obsolete scratch artifacts after acceptance. Active TerminalO3 builds were left untouched.

- [x] Navigator independently verified combined compiler evidence `native-tests-review-20260916T203517093149Z` and `wasm-tests-review-20260916T203739999840Z`: zero errors and unchanged source manifests in each gate. These gates cover the temporary combined stage, not live acceptance.
- [ ] Before runtime acceptance, repair owned stop cleanup to retain filmloop ScoreRef identity and end persistent active spans, and restore transition/dispatch state on owner-valid restart/network-movie completion and failure. Luna High compiler pilot owns this correction and the shared Cargo slot. Full host-notification and JS-Lingo cutovers continue in existing source-only stages.

- [x] Navigator ran combined `mise exec -- bun test ./scripts/flash-owner-lifecycle.test.mjs`: 11 passed, 0 failed, exit 0. Evidence summary `/private/tmp/dirplayer-combined-flash-suite-review-20260916.json`. This does not include the frozen queued goto/gotoAndStop regression; reconciliation assigned to combined pilot before browser acceptance.

- [x] Combined Flash queued seek regression reconciled and strengthened to assert underlying GotoFrame calls and arguments for playing/paused instances. Navigator rerun: 12 passed, 0 failed, exit 0; raw output `/private/tmp/dirplayer-combined-flash-suite-20260916-final.log`. Actual generated browser/Rust lifecycle gate remains pending.

- [x] Combined native205207 and WASM205402 compiler gates pass with zero errors and unchanged manifests. Native raw library run:452 passed/1 failed (BudAPI test nested RefCell borrow). After scoped reads, native205534 compiles clean and raw runtime remains452/1; failure now identifies incorrect baSetVolume test arity (volume belongs at argument1). Fixture correction assigned; production semantics and isolation/reset assertions must be preserved. No runtime acceptance yet.
- [ ] Notification cutover source checkpoint reviewed: lifecycle task still lacks reservation/tail RAII on cancellation; correction assigned. JS-Lingo unbounded fire-and-forget reentry rejected; pilot approved to preserve ticketed results/errors and explicitly reject unsupported synchronous reentry, with full-plan limitation retained.

- [x] Navigator verified native205805 and WASM205956 gates: zero errors/unchanged manifests; exact native library453 passed/0 failed/0 ignored.
- [ ] Actual Chromium gate `browser-isolation-runtime-20260916T210112Z` finished exit1/unchanged: generated lifecycle aliasing failure, nested input pass, public-play loop registration failure, then60s harness timeout; remaining fixtures unverified. Source review identifies omitted integration hunks in BrowserPlayerHandle: set_host_event_sink still calls JS synchronously, play/stop still only call DirPlayer flag setters, clear/reset capacity preflight missing. Assigned full reviewed-patch wiring audit and correction; no live integration/acceptance.

- [ ] Navigator rejected notification integration patch8ef6dc73: incorrect combined-before/stale-stage-after baseline deletes owned playback methods and backpressure/cancellation regressions. No integration performed; original-baseline-only delta requested. Lifecycle drain guard also requires attempted-event accounting before callbacks and rescheduling of restored tails. JS-Lingo setter remains on legacy error-only production path despite direct-helper fixture; owned property-set continuation assigned before integration.

- [x] Navigator integrated pilot-authored public handle repair after automatic approval had incorrectly classified pilot scope as cleanup-only; root apply succeeded under persistent implementation authorization. Bind/clear now queue lifecycle callbacks with preflight/rollback and PumpPending retained; reset preflights normal3 before mutation; play/stop call owned loop APIs. Independent pilot source review completed. Native211630 and WASM211853 gates zero errors/unchanged; exact native library453 passed/0 failed. Actual Chromium rerun started against frozen repaired source.
- [x] Revised JS-Lingo legacy+eval/session patches pass dry-run against combined; monotonic object IDs, registration error propagation and production typed setter/getter routing reviewed. Patches remain unapplied pending lifecycle browser gate.

- [ ] Chromium rerun `browser-isolation-runtime-20260916T211952Z` exit1/unchanged: unsafe-aliasing failure resolved and owned loops start. Remaining observed failures: terminal fixture incorrectly expects successfulBindings+1 retirements (initial binding included), public-play fixture lacks renderer, legacy isolation/tail fixtures assert synchronous sink delivery. Nested input passed; renderer script errors abort remaining fixtures. Coherent fixture/lifecycle boundary correction assigned, preserving assertions and real canvas setup. No browser acceptance yet.

- [x] Fixture repair native gate `native-tests-review-20260916T213623546270Z` passed with zero errors and unchanged source; exact compiled library runtime: 453 passed, 0 failed, executable unchanged. Browser acceptance remains pending.
- [ ] Review confirmed lifecycle/content ordering race: bind queues detached OwnerBound while direct notifications and PumpPending can deliver content first. Approved lifecycle-drain dispatch fence, final current-owner wake, immediate bind/notify regression, and fixture callback/container cleanup. WASM compiler gate runs before source edits resume.
- [ ] Notification-only rebase a2d3b0c6324e361b2bb1dac37e964cedebe22eddde134297eb8e33b8e0831df9 matches all 16 combined before hashes; not integrated. Reviewing frontend routing, session cast outbox ownership, legacy reset coverage and missing native regressions before acceptance.

- [x] Fixture repair WASM all-tests gate `wasm-tests-review-20260916T213758631080Z` passed with zero errors and unchanged source. These results precede lifecycle ordering repair and do not establish browser acceptance.
- [ ] Notification rebase review found real cross-player leakage: purge_invalid_pending may finalize B into a session-wide outbox subsequently published under A. Approved local operation outboxes and per-player publication, actual owner-route cached-cast tests, two-owner all-kind coverage, and a targeted stale-preload regression. Legacy reset Flash teardown must remain functional after dispatch migration. Patch not integrated.

- [ ] Reviewed and froze lifecycle ordering repair in combined: defer notification drain before and between callbacks while lifecycle work remains; final lifecycle completion wakes current owner and drains no-loop handles. Browser fixture repairs retain immediate/reentrant ordering assertions, JS-owned callbacks with Weak handle captures, detached fixed-size canvases and correct terminal retirement accounting. Actual browser gate `browser-isolation-runtime-20260916T214926Z` running; no acceptance claimed.
- [ ] Independent notification review caught absent application sink binding/event adapter and seven retained callers of removed wasm JsApi methods. Frontend adapter and Rust caller migrations assigned before integrating or compiling notification patch; cast reference adapter must preserve tuple payload shape.

- [x] Browser gate `browser-isolation-runtime-20260916T214926Z` compiled release successfully and completed with source unchanged: 8 fixtures passed (generated lifecycle, public play, nested input, host-tail preservation, BudAPI, FileIO, Multiuser, SysMenu), 1 failed (handle isolation callback-rebind). No broad browser acceptance/live integration.
- [ ] Remaining callback-rebind failure traced to synchronous notification delivery while exported set_debug_selected_channel still holds its receiver/fixture slot borrow. A callback requesting mutable rebind fails before retirement; proposed correction must schedule notifications outside exported receiver lifetime, with generated-JS coverage and detached error handling. Entry/per-item lifecycle fence remains necessary but insufficient alone.
- [x] Frontend adapter rebase34e81e91 reviewed against exact combined callbacks base d5156b5d: preserves existing Flash routers/imports/callbacks; root independently ran production adapter regression PASS. Not integrated pending complete notification bundle.

- [x] Post-lifecycle native gate `native-tests-review-20260916T215759280492Z` passed zero errors/unchanged; exact library runtime 453 passed, 0 failed, executable unchanged.
- [ ] Approved coherent exported notification boundary: owner-generation-coalesced detached drain helper, migration of BrowserPlayerHandle debug/snapshot/subscription producers, generated-JS mutable callback rebind regression, async callback error/attempted-once recovery. Combined pilot owns source; global notification/cast patch remains isolated and must rebase onto this helper before integration.

- [ ] Async export audit: installed wasm-bindgen0.2.108 macro code confirms async &self uses LongRefFromWasmAbi with RcRef anchor, retaining receiver borrow across Promise. Detached callback tasks alone do not make such exports reentrant. Auditing owned-value Promise wrappers and direct Rust callers before broad callback acceptance. Scheduler review also requires replacing stale generation markers without allowing old tasks to clear new markers.
- [x] Independent cast-owner patch audit found no remaining calls to removed drain_notifications or deleted JsApi methods in its patched snapshot; frontend tuple conversion covered separately. Legacy exported rendering debug producer still needs explicit captured-owner wake; bundle remains unintegrated and must rebase after scheduler handoff.

- [ ] Independent async audit identified nine BrowserPlayerHandle async exports, no direct Rust callers requiring borrowed signatures. Approved synchronous wasm Promise wrappers with owned inputs, post-await owner validation, generated pending-Promise reset/free regression, and owned command completion helper.
- [ ] Debug/snapshot migration review: current JS adapters route through global vmCallbacks; new deferred paths must route by captured owner. Script-instance snapshot ID0 currently reaches an unconditional None unwrap; correcting defined ID0 behavior with regression in the same boundary batch. These changes remain uncompiled/unaccepted.

- [x] Added exact-owner debug/script-error/datum/script-instance JS routes and real browser-harness forwarding. Root caught and corrected owner-last JS versus owner-first Rust ABI mismatch before build; independently reran owner-scoped-vm-callbacks.test.mjs PASS with owner-first calls, two owners, retired-owner dropping, payload preservation and exception propagation.
- [ ] Deferred snapshots now retain owner-qualified references rather than recyclable numeric IDs. Required same-generation reuse and generated-JS pending-Promise/rebind regressions remain part of source handoff. Native/WASM build gates wait for the complete frozen batch; previous453 native/8 browser pass results do not cover these edits.

- [x] Cleanup follow-up verified 456 GiB free; retained shared target7.3 GiB and source stage286 MiB. Pilots resumed with no new stages or copied build caches; CARGO_INCREMENTAL=0 remains enforced.
- [ ] Boundary source frozen for native all-tests compile `native-tests-review-20260916T223153255820Z`; collect WASM diagnostics next before batch fixes. Snapshot reference reclamation regression added, not yet executed. Generated-JS coverage still needs content-callback mutable rebind and controlled in-flight fetch reset/free; lifecycle-only rebind and one microtask delay do not prove these requirements.

- [x] Frozen boundary native gate `native-tests-review-20260916T223153255820Z` passed with zero errors and unchanged source; exact library artifact ran455 tests, all passed, binary unchanged.
- [ ] Same-source WASM gate `wasm-tests-review-20260916T223400282565Z` failed with5 diagnostics, all borrowed BrowserPlayerHandle IntoWasmAbi errors in browser_handle.rs imports. Pilot correcting fixture ABI; real generated-JS regression coverage assigned to template pilot.
- [ ] Navigator found SetSystemFontPath late-success ownership gap: player_load_system_font applies decoded bitmap via reserve_player_mut after await. Outer command Promise stale-owner rejection cannot prevent prior mutation. Owned font decode/application design assigned before claiming async owner isolation.

- [x] WASM fixture ABI repair passed `wasm-tests-review-20260916T223645731636Z`: zero errors, source unchanged. Rust registrations pass an optional JsValue rather than an unsupported borrowed exported class. Generated content-callback rebind/pending-Promise assertions are moving to the actual JS handle template; browser acceptance waits for equivalent coverage.
- [ ] Approved owned system-font decode/application fix and native stale/reset/foreign-owner application tests. Scope: font module, command caller, browser reset fixture caller; frontend pilot owns controlled-fetch generated tests. No builds until cohesive source freeze.

- [ ] Reviewed generated browser regressions in template; required corrections before execution: observe retirement on original sink, assert replacement bound before reentrant channel8, validate successful font fixture HTTP/image data rather than allowing failed fetch to masquerade as late success. Template pilot correcting; no browser acceptance claimed.
- [ ] Async wrapper audit added final explicit owner check after BrowserPlayerHandle::dispatch_flash_lingo evaluation. Source lib.rs SHA114769b7aad4d3cde66f86f5fa13ae85eefc82773de16ffe2f06119635b732e7; not compiled after this edit. Existing unqualified post-await movie-load failure callback remains in later notification migration scope.

- [ ] Owned font batch under native compiler gate `native-tests-review-20260916T224634917570Z`: fetch/decode returns bitmap without runtime mutation; final owner-validated short borrow installs font. Production command and browser reset caller migrated. Native regression compares font counter/map/cache/bitmap identity after foreign and stale attempts. Review corrected dimensions captured before ImageBitmap.close. Browser template corrections remain pending; no runtime acceptance claimed.

- [x] Owned font/Flash fence batch verified: native `native-tests-review-20260916T224634917570Z` zero compiler errors, unchanged source; exact library456 passed/0 failed. WASM `wasm-tests-review-20260916T224820768300Z` zero errors, unchanged.
- [ ] Reviewed corrected generated template SHA58d5877b7477a6d25f50a5466639363ddb462d192636d356a48eff4306646343; actual Chromium gate `browser-isolation-runtime-20260916T224917Z` running on frozen batch. Native font helper tests prove state preservation; browser verifies real generated class reentry, immediate/pending reset/free and stale-success rejection with validated image responses. No live integration or browser acceptance yet.

- [x] Browser gate `browser-isolation-runtime-20260916T224917Z` completed exit1 with source unchanged:8 fixtures passed,1 failed. Generated JS lifecycle/content mutable rebind plus immediate/pending reset/free and validated late-success font-response cases passed. Public play, nested input, host tail, BudAPI, FileIO, Multiuser, SysMenu passed.
- [ ] Remaining isolation fixture asserts second-player callback delivery synchronously after deferred producers; root found matching reset-subscription and post-disposal assertions. Approved one fixture batch adding bounded exact-event waits without weakening owner/no-replay checks; runtime source remains frozen.
- [x] Live408-file baseline rechecked against post-sysmenu-reset manifest: zero drift. Current combined batch changes26 baseline paths; integration remains pending complete browser acceptance.

- [ ] Fixture async-delivery repair reviewed at SHA7c7fb3b8e97a8ae2cbf183bea1353ba4e0da0f4d043496f3dad81a7194623937: wait for second channel9, post-disposal channel11, and reset score/channelNames only in post-reset event suffix. Existing owner isolation/error assertions retained. WASM recheck active; runtime sources unchanged since456-pass native gate.

- [x] Fixture repair WASM `wasm-tests-review-20260916T225506486768Z` zero errors/unchanged. Chromium `browser-isolation-runtime-20260916T225549Z` exit0/unchanged, all9 named fixtures passed including generated reentry/pending-font and handle isolation. Native runtime remains456-pass for unchanged runtime source.
- [ ] Frontend acceptance audit found missing real set_flash_scripted_access_pending capability (mock-only setter) and ES5 Map.values iteration. Targeted production-settings tsc confirms exactly2 Flash manager errors in `/private/tmp/dirplayer-flash-typescript-20260916.log`; initial whole-stage check was incomplete because stage omits unrelated frontend sources/config. Real owner-state capability design pending; no live integration yet.
- [ ] Notification bundle d1887eb26dcebe7e29e1c7a3fccc23f2bca3cd92f324e2067d5408e303679413 rejected despite17 matching before/after hashes: actual aftertree restored session-wide cast_operation_notifications/drain_notifications and omitted previously required owner-local/all-kind regressions. Pilot rebuilding from corrected cast-owner delta; no patch applied.
- [ ] Independent playback review identified lost stop cleanup when cancellation wins !is_playing branch and ignored cleanup error permitting queued replay. Bounded repair proposal/tests requested before edits.

- [ ] Approved real Flash readiness capability: per-generation Rc<Cell<bool>>, captured by BrowserPlayerHandle and non-owning BrowserFlashCapability; setter checks token liveness without session borrow or command-queue dependency. Reset clears old cell, creates fresh cell, and refreshes handle. Required TS setter/direct publication and ES5-compatible iteration are in progress; native/browser tests pending.
- [ ] Stop lifecycle repair design approved pending shared mod.rs ownership release: avoid cancelling/replaying attempted StopMovie/endSprite phases, propagate cleanup errors, suppress queued replay on failure, preserve reset/remove cancellation and owner-isolated transition flags. Pilot preparing deterministic boundary/error tests.
- [ ] Cast-owner rebase review found stale ChannelNamesChanged tuple signature incompatible with current unit variant; correction requested. Native notification coverage must account for newly retained datum/script-instance variants, not silently drop them or mislabel seven legacy kinds as all-kind. No rejected bundle applied.

- [x] Flash TS production changes remove missing-interface and ES5 iterator errors; root reran Flash host lifecycle suite12 passed/0 failed. Targeted tsc now reports only2 expected missing-method errors against the old generated BrowserPlayerHandle declarations; next WASM binding generation must verify the real new export.
- [ ] Flash Rust readiness wiring reviewed but uncompiled. Existing host mocks are insufficient: approved actual BrowserFlashCapability/BrowserPlayerHandle regression under held session borrow, two owners, stale reset capability, real FlashOwnerHost begin/complete adapter. Pilot owns lib/testing_browser/browser fixtures; stop pilot owns mod/session implementation.

- [ ] Corrected cast3file patch02891a45 reviewed structurally: per-operation outboxes, per-owner invalid-pending publication, deprecated session drain removed, current unit ChannelNamesChanged retained. Not integrated/compiled; depends on cohesive native host notification mapping.
- [ ] Guard review of rejected full bundle found Drop cleanup omitted final current-owner PumpPending/direct notification wake when lifecycle tail ends. Requested parity with ordinary drain plus queued-content guard-drop regression; no unverified guard patch applied.

- [x] Resume checks after disk cleanup: live408-file source baseline has zero drift; Flash host Bun suite12 passed/0 failed and Node owner callback routing passed (two owners plus retired-owner rejection). Shared Cargo target remains7.3 GiB; no duplicate build caches created.
- [ ] Real Flash capability regression strengthened with distinct-cell and unaffected-neighbor assertions plus Rust-observed production FlashOwnerHost transitions true/false/true/false; source reviewed, native/WASM/browser verification pending stop-lifecycle source freeze.
- [ ] Full live ownership audit remains failing with757 findings in /private/tmp/dirplayer-ownership-resume-audit.log. These are static migration findings, not compiler errors; no stage3 completion claimed. Stop lifecycle and isolated notification guard corrections remain in progress.

- [ ] Guard patch0c3b98e9 rejected during review: first delivery owner may be retired in a mixed reset/rebind lifecycle batch, so fencing lifecycle rescheduling on that token strands later retirement/bind events. Required correction: always drain lifecycle tails, then capture current owner for its own content drain and revalidate before PumpPending; add mixed-generation regression in actual browser fixture. No patch applied.
- [ ] Actual Flash Lingo audit confirmed unqualified host calls in manager/sprite/FlashObject handlers and synchronous JS calls under VM borrows. Approved bounded owner-qualified frontend bridge routes/tests first; typed deferred evaluator/driver Flash request migration remains required.

- [x] Combined stop/readiness batch frozen at mod.rs1aaa4ddd and session.rs27c69cd2: native all-test compilation native-tests-review-20260916T233020287266Z zero errors/unchanged; exact native library binary460 passed/0 failed (runtime-vm_rust summary). WASM all-test compilation wasm-tests-review-20260916T233200556015Z zero errors/unchanged.
- [ ] Browser verification of actual Flash capability remains pending frontend-only bridge source freeze. Stop tests currently prove phase helper retention, stale-owner cleanup propagation and replay bookkeeping; real same-owner callback failure/finalizer replay and actual pending-callback reset are not yet proven.
- [ ] Guard corrected patch008183f8 source-reviewed (mixed lifecycle rescheduling, current-owner final wake, Sender Option type fixed); not applied or compiled. Browser HTML auto-discovers exported test_ functions; include browser_host_event_guard filter on its eventual runtime gate.

- [x] Actual Chromium browser-isolation-runtime-20260916T233318Z exit0/unchanged:10 fixtures passed, including test_browser_handle_flash_scripted_access_owner_capabilities. Targeted production-settings TypeScript check with actual newly generated WASM declarations also exits0 via /private/tmp/dirplayer-generated-capability-tsconfig-20260916.json. This proves real readiness export and host transitions, not full Lingo handler migration.
- [ ] Applied source-reviewed guard patch008183f8 to combined lib.rs (after9806b66f); compiler/browser guard regressions pending.
- [ ] Frontend owner-first route batch has pilot13-pass Bun result, but review found shared numeric pinTarget cross-owner cancellation and disposed-host strong registry leak. Approved per-host seek state, exact-entry terminal cleanup, duplicate-owner rejection and regressions; label lookup remains explicit legacy unsupported(-1), not claimed implemented.
- [ ] Approved combined real stop callback failure/finalizer regressions (mod/events) and per-owner cast outboxes plus portable native notification payloads/mailbox/explicit dispatch (cast/session/host_events/js_api). Shared-file ownership assigned; no new stages/caches.

- [x] Navigator reran Flash host suite17 passed/0 failed and targeted generated-declaration TypeScript exits0 for frontend checkpointd96cbc2f. All four shared-target incremental directories0 bytes.
- [ ] Frontend review then found stale registration teardown can delete replacement instances sharing the same ownerKey: require exact host/instance identity before and after reentrant remove hooks. Correction/tests in progress;17-pass checkpoint is not final acceptance.
- [ ] Native DTO review requires typed datum values, full member/channel/score snapshot fields, and owner-isolated mailbox capacity; metadata-only/debug-string payloads do not fulfill native inspection. Requested correction while implementation is in progress; no native notification acceptance claimed.

- [x] Frontend teardown checkpoint62590ced: navigator Bun18 passed/0 failed and generated-declaration TypeScript exits0; exact host/instance cleanup protects reentrant same-key replacement.
- [ ] Further actual create-load race identified: owner generation alone cannot reject a late load after same-owner/same-sprite replacement. Approved frontend per-instance generation and production pending-load race regressions; Rust typed Flash requests will carry that generation in later integration.
- [x] Review caught native dispatcher range replacement deleting WASM implementation before any compiler run. Pilot restored baseline js_api0242b168; navigator independently confirmed the complete115738-character WASM impl is byte-for-byte preserved while native changes continue under their own cfg block.
- [ ] Stop callback batch uses real evaluator StopMovie/EndSprite counters and production finalizer; requested Abort no-error reporting plus exact report counts and stage/filmloop exit assertions. Next combined compiler pass waits for stop/native payload source freeze.

- [x] Recovered frozen combined native compiler gate `native-tests-review-20260916T235626781395Z`: exit 0, zero errors, unchanged sources. Matching WASM gate `wasm-tests-review-20260917T000301538354Z`: exit 0, zero errors, unchanged sources.
- [ ] Exact compiled native library execution: 462 passed, 2 failed (cached cast notification ordering and cross-player purge notification fixture). Failures inspect only the legacy pending notification queue despite production bounded host mailbox routing; second fixture also needs to respect the two-cast preload barrier. Pilot correcting observations through the production drain; runtime acceptance remains pending.

- [ ] Follow-up native gate `native-tests-review-20260917T000550787374Z` compiled unchanged with zero errors; exact runtime 463 passed, 1 failed. Cached snapshot regression now passes via production drain. Remaining purge fixture incorrectly expects preload completion after retirement transitions to Idle; retain production cancellation semantics and exercise a real B cast mutation notification instead.
- [ ] Browser guard fixture now retains the session borrow across a timer turn before asserting deferred tail delivery; actual browser verification pending. Next Flash migration requires a typed deferred request for real Lingo getter/setter/call plus atomic owner/instance-generation bridge result; owner-current JS helpers alone are insufficient.

- [x] Final cast fixture/native regression gate `native-tests-review-20260917T001116703904Z`: compiler exit 0, zero errors, unchanged sources; exact compiled library 464 passed, 0 failed. B notification is generated by normal CastManager::load_from_dir, then observed through merged owner-bound drain during A-triggered purge; cancellation stays Idle. Final WASM and strengthened browser guard runtime verification pending.

- [x] Final WASM test compiler gate `wasm-tests-review-20260917T001228191421Z`: exit 0, zero errors, unchanged sources.
- [ ] Browser gate `browser-isolation-runtime-20260917T001256Z`: exit 1, unchanged sources. Optimized WASM build completed; real guard tests logged success but harness caught concurrent command-loop panic at commands.rs:713 while new fixture deliberately held a session borrow across a timer. Acceptance rejected. Isolate guard retry fixture from spawned command loop while retaining real timer retry; actual exported reset/rebind fixture remains.
- [ ] Approved next disjoint batches: checked/bounded native DTO conversion and owner-local error slot; atomic generation-aware Flash JS bridge. Rust real Lingo deferred Flash migration remains design-only, not implemented.

- [x] Navigator verified generation-aware Flash bridge: 28 Bun tests passed (log `/private/tmp/dirplayer-generation-envelope-tests-20260917.log`), TypeScript exit 0 with `/private/tmp/dirplayer-generated-capability-tsconfig-20260916.json` mapping current generated WASM declarations. Legacy live declarations omit the newer readiness setter and are not valid for this stage. Bridge validates exact owner/instance before and after calls, property access and error coercion; not-ready is explicit without host side effects. Real Rust Lingo producer/executor migration remains pending.
- [ ] Guard retry fixture now uses isolated RuntimeSession/player/channel without spawning unrelated command loop; timer-held borrow, actual guard cleanup and ordered sink delivery remain. Browser rerun pending combined source freeze. Native snapshot batch under implementation/review; no compiler or runtime acceptance yet.

- [ ] Rust Flash migration implementation assigned as a coherent batch: typed internal requests, actual getter/setter/call and initial binding producers, owner/instance-generation continuation validation, no VM borrow across host calls or JS decoding. Session continuation edits require serialized handoff after DTO pilot; callback-origin generation contract under separate frontend review. Initial not-ready binding must retain first returned generation and retry at that generation, never silently rebind.

- [ ] Native DTO review corrected FilmLoop score source to explicit member data.score; main stage uses shared explicit-score helper. Bounded score payloads/channel script validation added; acceptance still pending actual allocator/member regression matrix and compiler/runtime gates. Session file ownership transferred from native DTO pilot to Rust Flash pilot at SHA256 5d3841404ca72655f3d6f80e126ed34ef4082b7fc0862d9fd2d477904e41ee5f; native tests continue in js_api only.
- [ ] Callback audit confirms forked Ruffle AVM1 still uses global LINGO_CALLBACKS; true callback-origin generation requires per-instance Ruffle registration/producer migration, not decorating the callback with current TS state. Existing dirty Ruffle owner/localconnection changes preserved; no Ruffle mutation yet approved.

- [ ] Approved shared Flash binding authority: runtime-owned per-player counter/map, separately borrowed capability state; TS reserves before async create and invalidates exact captured generation on teardown. Rust validates same state after JS result decoding before allocation. This closes same-owner JS host re-registration generation alias and post-decode replacement races. Callback registry migration in actual Ruffle must be perplayer; exact source-only proposal pending, no full checkout/build-cache copies.

- [ ] Native DTO regression matrix authored in js_api native tests: foreign nested refs/symbols, stale script/ancestor, cycles/depth/width, FilmLoop-vs-stage score, representative media fields, reset/remove error/mailbox cleanup and sibling capacity. Source reviewed; obvious VecDeque fixture and Array arity errors sent back before compiler. No runtime acceptance.
- [ ] FlashBindingState implementation underway (runtime-owned monotonic safe-integer counter, sprite-generation map); JS host uses reserve/invalidate/check capability methods. Review requires checked sprite range, exact expected invalidation, no generation overwrite on not-ready retry, and post-decode validation before allocation.

- [x] Frozen Flash/native DTO aggregate diagnostics collected: native-tests-review-20260917T004814977267Z reports28 diagnostics, WASM wasm-tests-review-20260917T005323195652Z reports14; both source manifests unchanged. Counts include duplicate library/test diagnostics. Grouped repairs assigned by file ownership; runtime acceptance pending.
- [x] Navigator independently reran Flash lifecycle suite after removal of production binding-authority fallback:30 passed,0 failed. Runtime Rust/native/browser integration remains unverified for this batch.
- [ ] Review corrections required before Flash acceptance: preserve returned object sprite binding, reject retired argument handles, bound recursive argument conversion and pre-allocation array size, validate numeric/date and sprite conversions. Ruffle per-instance callback registry exact API proposal remains under review.

- [x] Grouped compiler repairs verified: native gate `native-tests-review-20260917T010019445803Z` and WASM gate `wasm-tests-review-20260917T010148550883Z` both exit0/errors0/source unchanged. Native DTO imports/integer conversions and Flash explicit owner/type/pattern repairs compiled together.
- [ ] Exact native binary runtime failed: two debug-output64KiB assertions, then native wasm-bindgen abort. Isolated snapshot suite5passed/2failed; serial full run locates abort at `native_notification_drain_preserves_all_owned_kinds_for_two_players`. Logs `/private/tmp/dirplayer-native-snapshot-isolation-20260917.log` and `/private/tmp/dirplayer-native-serial-20260917.log`. Grouped repair assigned; no runtime acceptance or live integration.
- [ ] Ruffle per-instance callback proposal approved for nine selected source files only, staged within existing review workspace; no full checkout/cache copy and no live Ruffle changes. Exact receiver API captures immutable owner/sprite/generation, removes global registry, drops registry borrow before JS conversion/dispatch. Integration awaits source review and compilation.

- [x] Native runtime repair checkpoint `native-tests-review-20260917T010754319546Z`: compiler0errors/source unchanged; exact native binary472passed/0failed/executable unchanged. Streaming debug formatting keeps truncation marker inside64KiB; two-owner notification fixture has a real owner-local member and native-safe error diagnostics. Missing-member rejection preserved.
- [x] Browser template now forwards all four generation-qualified Flash exports to the production bridge; source hash44a58acb293073b52274b004645f0230c8bdb84e70d6cf9bd846cd667e7b0423. Browser runtime acceptance pending.

- [x] Checkpoint WASM compiler `wasm-tests-review-20260917T010910900494Z`:0errors/source unchanged. Browser `browser-isolation-runtime-20260917T011001Z`:exit0/source unchanged,12actualruntimefixtures passed/0failed, including both host-event guard cases. These checks do not yet exercise the pending full Flash producer/result-mode/callback migration.
- [ ] Ruffle callback registry source patch reviewed with four focused registry tests authored, beforehashes match all9livefiles; tests not compiled/run and patch not integrated. Next acceptance requires coordinated producer/frontend/VM dequeue ABI and actualRufflebuild.

- [ ] Post-browser targeted TypeScript check against generatedWASM found TS2802 at flashPlayerManager.ts1318/1572: Mapspread/iteration incompatiblewith existingES5target. Approved Array.from entrysnapshot repairs; target unchanged. Native472 and browser12 checkpoints remain verified; TS acceptancepending.
- [ ] Next Flash ownedrequest batch approved: both getVariable producer forms/resultmode, pendinginitialbinding without fabricatedunboundhandles, generationqualifiedreadiness, checkedJSONargs and crossspritedatumrejection, realRustauthoritytests. Callback ABI cutover remains separate andRufflepatch isolated.

- [x] ES5 Mapentrysnapshot fixes verified with targeted generatedWASM TypeScript check:exit0, managerhash86d743bee89831b6347f3cf5827c36750bd2071105aa4933adcb4403f63855f2.
- [ ] FocusedFlash rerun after template/ES5 changes:26passed/4failed, all delayedimport subsequentregistration/lifecycle cases; log `/private/tmp/dirplayer-flash-es5-tests-20260917.log`. Root assigned grouped cause investigation; do not claim current30testpass. Prior native472/browser12 remaincheckpoint evidence, not evidence for ongoingFlashchanges.
- [ ] ApprovednativeScriptmember snapshot parity and portableShapefields in nativejs_api only; actualfixtures, boundedinput/output and expliciterror/truncation required. WASM snapshotimplementation preserved.

- [x] FocusedFlash failurefamily fixed via explicit temporarybundle module-evaluation barrier beforecleanup, no productiontiming workaround. Navigatorrerun30passed/0failed: `/private/tmp/dirplayer-flash-import-barrier-tests-20260917.log`; testhashc20331f81fb195e6906f72b23e1240b3d2eba9136da52cad8bd79f32d37a4566. ES5managerfixes TypeScriptpass.
- [ ] ApprovedJS/Rust ownedFlash contract: trailingreturnAsObject onbothgetroutes; `dirplayer_isFlashInstanceReadyOwned(ownerKey,sprite,generation)` returns `{ok:true,generation,ready}` or typedfailure. Exactinstance/sharedauthoritychecks and ownerrevalidation afterdecoding required; no hostget/callsideeffect forreadiness. NativeScript/Shape andFlashbatches underway; latestpassingchecks applyto precedingfrozencheckpoint.

- [ ] Ongoing Flash review rejected response-driven generation adoption: invalidatedauthority mustnot be resurrected fromlatehostresponse; initialBindGet maycaptureonlyexistingcurrentgeneration. RetryNotReady mustrejectchangedgeneration instead ofoverwritingcapturedvalue. RealRuntimeSession reset/capabilitytests required beyond isolatedstatetests.
- [ ] NativeScriptsnapshot review requires aggregateargument/span/map/text budgets, preflightliteral/name sizes beforedecompilation, checkedjump arithmetic beforeto_bytecode_text, and explicitmalformedargument-name errors. Draftnotaccepted; compilerbatch pendingpilotfreeze.

- [x] Navigator verified frozen owner/generation readiness JSbridge:32Bunpassed/0failed (`/private/tmp/dirplayer-flash-readiness-tests-20260917.log`), targetedgeneratedWASM TypeScriptexit0. Readiness tests cover replacementduringreadygetter anderrormessagecoercion, independentowners, nohostget/call/queueeffects. Managerhash43b7e34debfe27a8a95d2d9f7c0e4a8c065999b11d2cae8c6d24d3ec2cfd1795. Gettransport staysraw; object/scalarmode semantics require actualRustdecoder tests. CurrentRustbatch notyetcompiled/frozen.

- [ ] FrozenaggregateFlash/ScriptShape compilerpair: native`native-tests-review-20260917T013027906666Z`2duplicatedpathmove diagnostics; WASM`wasm-tests-review-20260917T013107348248Z`6diagnostics fromsamepathmove andu64/f64readinesscomparison; bothsourcesunchanged. Groupedrepairassigned, nativeDTOcompileswithoutreportederrors; runtimepending.
- [ ] FrozenFlashreview identifieddriverbytecode setter stillcallingapply_set_prop withmutableRuntimeSessionborrow; mustlowerintoowner-boundInternalInvocation withSetPropertyeffect. Setterpostdecodeowner/currentgenerationvalidationmissing; evaluatorinitialBindGet completiongenerationhandoff underaudit. Resettest incorrectlyassumedold/newnumericgenerationdifferent; actualowner/stateisolation mustbeasserted.

- [x] GroupedFlash repair compilerpair verifiedzeroerrors/sourceunchanged: native`native-tests-review-20260917T013431341230Z`, WASM`wasm-tests-review-20260917T013618618909Z`. Exactnativebinary476passed/1failed; newScriptShape fixtureexpectsnonemptyLingo fromPushInt8+Ret. Repairassigned; runtimeacceptancepending.
- [ ] Productionevaluator audit confirmedFlash requestsrequeue unchanged inexecute_eval_request/finish_pending_eval_request_turn withoutcallinghostexecutor. complete_typed_async_eval unused; editingit alonecannotfixruntime. Approvedsharedowner-boundFlashexecutor usedbybytecodedriverandevaluatorpump, resumesoriginalEvalId/action withvalidatedresult; actualrequest/reset/replacement regressionsrequired.

- [x] Recovered terminal native gate `native-tests-review-20260917T014447448705Z`: exit101, one E0308 in session_callback_tests.rs:925, source unchanged. Pilot replaced manual Flash completion test with production pump + result receiver + empty-queue assertions; rerun pending browser fixture freeze.
- [x] Disk follow-up verified all66 duplicate-cache targets absent; removed eight abandoned childhood-redux parity staging directories (2.00GiB). Existing shared target retained; no new stages/caches, CARGO_INCREMENTAL=0 for verification.
- [ ] Real browser evaluator regression approved: initial owned binding, generation-qualified property read, replacement during host response rejection, and fresh replacement binding. Current native/WASM/runtime acceptance remains pending.

- [x] Native compiler gate `native-tests-review-20260917T015813515107Z` zero errors, source unchanged. Exact binary477 passed/1 failed: real Flash pump test receiver closes without result. Script/Shape snapshot fixture now passes.
- [ ] Root traced receiver failure to single-request extraction omitting InflightEvalRoute registration, unlike plural extraction. Approved consistent owner/action/sender registration plus success/error/reset regressions. WASM `wasm-tests-review-20260917T015933294847Z` six duplicate missing-Cell-import diagnostics; fixture import repaired, verification pending.
- [x] Pending-publication Flash reservation bridge independently verified:34 Bun tests passed and generated-capability TypeScript exit0. Readiness exposes exact existing reservation, rejects reentrant invalidation/replacement; no generation adoption. Log `/private/tmp/dirplayer-pending-reservation-tests-20260917.log`.

- [x] Final Flash/scheduler native gate `native-tests-review-20260917T021707569805Z`: compiler0/source unchanged; exact native library480 passed/0 failed, executable unchanged. Real list-object count continuation proves successive evaluator actions deliver result3 and retire old routes; sibling reset receives Abort once each.
- [x] Matching frozen WASM gate `wasm-tests-review-20260917T021822298126Z`: compiler0/source unchanged.
- [ ] Final browser gate `browser-isolation-runtime-20260917T021843Z` running, including new real Flash evaluator binding/replacement fixture. User requested pause after current Flash work; no JS-Lingo migration to start after this checkpoint.

- [x] Final browser gate `browser-isolation-runtime-20260917T021843Z`: exit0/source unchanged;13 actual Rust browser fixtures passed, including `test_flash_owned_evaluator_binding` (BindGet, captured-generation Get, replacement rejection, fresh replacement bind/read).
- [x] Current Flash checkpoint verified in existing combined review stage: native480/0, browser13/0, frontend34/0, native/WASM compiler0, TypeScript0. Hashes and gate references: `integration-evidence/flash-verified-pause-20260917.json` in review stage.
- [ ] PAUSED at user request after current Flash verification. Live integration remains pending; no JS-Lingo registry migration started. Full native-player plan remains incomplete. Resume only when requested.
