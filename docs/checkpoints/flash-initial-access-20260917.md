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
