# Retained-reference reclamation acceptance

Stage 2.1 is accepted against `d8f868fdc7546cf36c594719caf656598451e9c9`
plus the three allocator tests identified by the patch/source manifest. The
source snapshot contains 419 tracked files exported from that revision; only
`vm-rust/src/player/allocator.rs` receives the test-only patch. No dependency or
build tree was copied. This isolates acceptance from concurrent child Flash work.

Fresh native compilation succeeded. Each of the three named tests passed alone,
and the exact resulting library executable passed **573 tests, 0 failures**.
`result.json` records the executable hash, commands and exits; `source-manifest.json`
records the revision, patch hash and all input-file hashes. All input hashes
were rechecked after execution. The live allocator matches this tested source.

The new tests use real allocator drop queues: chunk-owned source reclamation,
retained child survival, timeout callback/target/script-datum reclamation, stale
drops across reset with reused IDs, and equal-key but distinct-owner neighbors.
The full suite also executes the existing reference rejection, symbol, bitmap,
explicit transfer, iterative duplication and scope invalidation regressions.
The [reference audit](../retained-reference-audit-20260917.md) maps these checks
and reviewed consumers to the Stage 2.1 exit requirements.

Production source is unchanged from the accepted WASM/browser checkpoint; the
three additions are ordinary `#[test]` unit tests, executed here natively. WASM/browser gates were not rerun for this
test-only change. Child Flash lifecycle, timer host ownership, remaining manager
cutovers and complete Stage 2 acceptance are not established by this result.
