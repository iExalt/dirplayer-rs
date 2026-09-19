# Timer runtime stabilization receipt

> Cleanup (2026-09-19): temporary paths and commands below are historical provenance; the temporary worktrees and build outputs have been removed. Retained archives now use repository paths. See [cleanup index](../../tmp-cleanup-20260919/INDEX.md).

## Frozen source

- Accepted cumulative base: `2812295899c5cd8e255f000a0df25f3dc78f1682`
- Stabilized source commit: `fff7dcc3804d857c0bc1b48570399bcea7cee5c8`
- Stabilized source tree: `62d55e06906ab24cfad683342b1d49d1e142fd22`
- Binary patch SHA-256 from the accepted base: `12e7dcb6ca5d513e3b84dd0d087014542f5dc30fb36108f91f046363401e73ef`
- Branch/worktree: `codex/native-dirplayer-readiness` at `/private/tmp/dirplayer-native-readiness-cumulative-20260918`

The source commit changes four paths:

- `vm-rust/src/player/commands.rs`: keeps timeout lookup, datum allocation, and callback dispatch on the captured owner; adds a real nested/root `TimeoutTriggered` regression that checks the TimeoutRef argument and the second-slot `Void` control.
- `vm-rust/src/player/timeout.rs`: preserves exact-name priority and makes ambiguous case-insensitive fallback deterministic.
- `vm-rust/src/player/testing_browser.rs`: corrects the scheduled reentry incarnation, prevents automatic one-millisecond replacement races in the explicit nested lifecycle fixture, and stores the argument before publishing its completion count.
- `vm-rust/tests/browser_templates/dirplayer-js-api.js`: permits `current` probes for tracked nested VM owners while retaining root-only schedule/clear callbacks.

`eval.rs` diagnostic refactoring and `mod.rs` formatting residue were restored byte-for-byte to the accepted base before the source commit. Temporary production traces, the allocator-churn hypothesis, and the incomplete two-ExtCall surrogate are absent. Lead and Luna-high review found no other source path outside this boundary. `git diff --check` passed.

## Root cause and contract

The retained browser trace showed the same TimeoutRef datum (`5134`) at owned event entry, callback start, handler setup, and scope argument installation for owner `2:2:1`. The apparent retained `Void` came from the synthetic observer: the driver yields after each synchronous opcode, while the observer incremented its completion count before executing `GetParam` and `SetGlobal`. The browser poll therefore read the argument slot too early. The fixture now writes the argument first and increments the completion count last.

No generic allocator or interpreter ownership change was needed. The production correction remains narrowly scoped to owner-bound timeout lookup/allocation/dispatch. Its native regression proves the required root and nested behavior through the real `TimeoutTriggered` command path.

## Focused validation

All commands used the shared `native-stage2/target`, offline Cargo mode, and repository-managed tools through `mise`.

| Check | Result | Canonical log SHA-256 |
| --- | --- | --- |
| `cargo test --lib timeout` from `vm-rust` | 16 passed, 0 failed, 620 filtered | `0546345677b5bd85b51d5d08372880a63b6fee308088bc3149a2358cb15f39fb` |
| `npm test -- --watchAll=false --runInBand src/services/flashPlayerManager.test.ts` | 18 passed | `f2b2abf29f393ef3554c575e35c77f66690abede6832068ee2ead60f803adcbb` |
| `bun test scripts/flash-owner-lifecycle.test.mjs` | 38 passed | `7efdcbade049261eb8a2ea63a478e7b911cc432999323a55fccbbf7318655d5a` |
| `cargo check --target wasm32-unknown-unknown --lib` from `vm-rust` | passed | `e5b8d6b37ca1bf45ec9959c788e6a056f0cd64bf539c0b98589a134ff48b30df` |
| Four-case production browser run, one worker | `browser_owner_timer_lifecycle`, `flash_initial_access_before_reservation`, `flash_owned_evaluator_binding`, and `nested_flash_owned_fixture` all passed; 4 passed, 0 failed | `2ba4e8ca70409bce578cf8d58b35a2b7970518ff83a34289e780a05714a0051d` |

The four-case browser command used `E2E_FILTER=browser_owner_timer_lifecycle,flash_initial_access_before_reservation,flash_owned_evaluator_binding,nested_flash_owned_fixture`, `--timeout=180000`, and `--workers=1`. The exact-source build completed, but its first browser launch found the ignored Playwright-cache symlink had been removed during cleanup; no test case ran. Restoring that local symlink and invoking Playwright against the already-generated runner passed in 23.8 seconds, without another build or source change.

The first cleanup-era WASM library check correctly failed with four `E0425` errors after `child_owner_key_a` was removed with temporary diagnostics. Restoring that required fixture owner key produced the green library check above. The exploratory `cargo check --tests --target wasm32-unknown-unknown` remains a configuration-specific non-gate: it reports 14 `E0599` errors in unit-test-only code that calls native-only helpers returning `()` under wasm configuration (11 accepted sprite-test sites and 3 timer-test sites). The production WASM library and browser artifact compile and run successfully.

## Artifacts and retained evidence

- WASM runner: `25cadf140259f36849015cbf398e134c4af37a914d34692d82176181361a7038`
- Generated WASM JavaScript: `63a4220adfdfd4d5c0bca9926e54acbe038679fda4e583258db12b65834ce741`
- Generated DirPlayer browser API: `8b7eee1e39953abb12487cf7fb2b876b9e6f9069d34408fcec16d8c618929526`
- Generated frontend manager bundle: `a78296eb40646854d8708148d9fea8c79ddfdb8d946cc88cdcafb8e6f32390ad`
- Complete runtime log archive: `docs/checkpoints/native-readiness-20260918/timer-runtime/artifacts/dirplayer-native-readiness-timer-runtime-logs-20260918.tar.gz`, SHA-256 `1fa550cf51bcbee5de218887292b903305a32b2dd549d719b58e5d90ace8cd66`
- Expanded logs: `/private/tmp/dirplayer-native-readiness-timer-runtime-logs-expanded-20260918`
- Frozen browser runner: `/private/tmp/dirplayer-native-readiness-browser-runner-fff7dcc3`
- Shared native/WASM target and build workspace: `/private/tmp/dirplayer-native-readiness-native-stage2-fff7dcc3`

The original dirty checkout remains at `fa80cb175625ac0e72022ff399247623f66b00cd` with binary diff SHA-256 `15588ab0bdbda53db608b9b60bf06dde21bb55836b167f68fc4480bc3158593e`; its standalone nested Ruffle changes remain untouched.

## Readiness outcome

The owner-bound timer contract is stabilized and now has focused native, frontend lifecycle, WASM library, and production browser evidence from one frozen source identity. A user can run nested and root timers through the existing browser/Flash integration without the observed early-read failure and can distinguish stale/replaced/reset timer incarnations deterministically. The next obstacle is the separately owned combined production/browser baseline gate; this checkpoint does not claim that broader gate or native backend implementation.
