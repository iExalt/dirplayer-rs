# Flash initial-access and value-transfer checkpoint — 2026-09-17

Publication checkpoint at the user’s commit-and-push request. This is incomplete
Stage 2 work, not an accepted Stage 2.6 milestone or a completed native player.

The patch prepares owned Flash load, resize and unload actions outside player
borrows, carries instance generations through frontend registration delay, and
adds initial-access preparation and an authored SWF browser fixture. It also adds
explicit owned value snapshots/import with symbol remapping, DAG alias preservation,
and rejection of unsupported references and cycles.

## Verification at publication

- Native test compilation: compiler-5 exit 0.
- Focused value-transfer tests: 8 passed, 0 failed (compiler-4 artifact).
- Frontend lifecycle tests: 38 passed, 0 failed.
- Final native suite: 543 passed, 1 failed, exit 101. The failing test is
  `player::session::callback_tests::evaluator_flash_request_uses_owned_transport_and_consumes_native_failure`,
  asserting the native unsupported-host error message at session_callback_tests.rs:914.
  Restoring the established detached-action error text did not resolve this failure.
- Both repository diffs pass `git diff --check`.
- Fresh WASM compilation, TypeScript validation and production browser fixture
  execution have not been completed for this patch. The browser template changed
  after native compilation; native compilation does not validate that template.

Local receipts: `/private/tmp/dirplayer-stage2-6-validation/`, specifically
`compiler-5/`, `value-transfer/`, `frontend/`, and `native-final-2/`.
These temporary paths are local evidence and are not portable repository artifacts.

## Remaining acceptance

Resolve the native regression; validate production browser initialization, reset,
replacement and captured-cast races; complete WASM/frontend ABI checks; and
establish actual nested Flash host registration and retirement. Other ownership,
async cancellation, callback-origin and legacy-adapter Stage 2 gates remain open.
The pre-existing Ruffle working-tree changes are excluded from this publication.


## Follow-up review after publication

The native unsupported-host test passed after installing a matching Flash member
and sprite; the published fixture lacked that setup. Subsequent validation is
tracked separately under `/private/tmp/dirplayer-stage2-6-resume/`.

A direct production JS bridge reproduction found a separate registration-reentry
failure: queued Load/Resize/Unload delivers only Load when its callback registers
a replacement callback table synchronously. The outer flush stores its tail
after the replacement registration has already attempted to drain it, so a
third registration is required to make progress. The existing frontend test
included that third registration and did not detect the failure. Local evidence
is `rebind-review/` under the resume directory (observed exit 1). This needs an
iterative per-owner drain and a regression requiring the replacement registration
to receive the tail without a further registration. Browser compilation alone
cannot establish correctness of this queue boundary.


The resumed production-manager browser run completed with **12 passed, 2 failed**
(exit 1; `browser/output.log` under the resume directory). The initial-access
fixture's primary failure was `Flash host operation failed: missing-instance`
during startup; its later undefined `flashInitialValue` error was secondary.
The older owned-binding fixture failed the new captured-cast check. These are
failed diagnostic results, not browser acceptance. The first failure requires
tracing prepared-generation publication through the harness registration delay
and production host path before choosing the fix.


The subsequent queue repair advances to the current callback registration for
each action and processes reentrant enqueues in FIFO order without replaying the
consumed prefix. A per-drain 128-action budget terminates self-feeding callbacks;
reset/overflow retirement rejects later prepared notifications. The frontend
pilot reports 38 lifecycle tests passed, zero failed. The navigator independently
reran the original single-rebind reproduction successfully and verified the final
source hashes:

- `dirplayer-js-api/index.js`: `aef409c5dbaa8d6529564758b28c4a73e3ac0a7622fecfdeba65e8155d05de37`
- `scripts/flash-owner-lifecycle.test.mjs`: `03282b37f96b0fe8920487eb34493e2743d430e51098283f19e741b67a2ef2ca`

This frontend result does not replace the pending fresh native/WASM/browser
validation of the combined Rust, harness and duplication batch.


The combined native run subsequently passed **554/0**, including duplication
**6/0** and Flash action/binding tests **8/0**. WASM test compilation passed and
all 378 recorded Rust hashes matched. Receipts are under `native-verified/` and
`wasm-final/` in the resume directory.

The subsequent `browser-final/` run still reports **12 passed, 2 failed**. Its
initial-access fixture fails *before* its Flash read because the newly added
`global` declaration is interpreted as `global(Void)`, an unsupported handler.
The controlled owned-binding fixture rejects its response generation. These
results do not yet prove the host-registration ordering repair. The current
browser filter also omits `test_js_object_owner_bridge`; that existing accepted
fixture must be included in the combined regression evidence.

## Publication follow-up

The omitted `test_js_object_owner_bridge` passed separately (`browser-js-final/`,
exit 0). After correcting the unsupported fixture declarations and capturing the
actual binding generation, the fresh `browser-flash-focused-2/` run reports
**one passed, one failed**: controlled owned binding passes, while real initial
access still fails with `Flash host operation failed: missing-instance`.

The harness removes and re-adds its player without advancing the session
generation, reusing the serialized owner key. The browser reset tombstone then
rejects the replacement's prepared Load. The proposed generation repair has not
been applied; clearing the tombstone would allow stale actions to target the
replacement. Production `reset_owned_core` rotates ownership separately.

This publication is an incomplete checkpoint. The combined 15-fixture browser
gate and root initial-access acceptance remain open. The latest browser-only
fixture edits postdate the native/WASM source manifest above. Staged evaluator
cancellation work under `/private/tmp/dirplayer-eval-cancellation-review/` and
the existing Ruffle working-tree changes are excluded.
