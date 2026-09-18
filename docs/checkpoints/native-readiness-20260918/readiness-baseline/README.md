# Combined readiness baseline receipt

## Outcome

This checkpoint is the production-build and regression baseline for the next
single-session native probe. It combines the cumulative accepted source, the
stabilized owner-bound timer contract, the production WASM/frontend package,
and the existing BrowserOwner regression set. It does not claim Stage 2 or
Stage 3 completion, a licensed-movie campaign, or native backend support.

The first combined browser run exposed a browser-fixture observation race. The
nested timer observer published its count immediately before the handler's
`Ret`, so the fixture could start Flash access while one child command was
still pending. The source checkpoint now enqueues and awaits an owner-checked
FIFO idle barrier at each timer-observer boundary. No production evaluator,
allocator, timer, or ownership contract changed.

## Frozen source

- Branch/worktree: `codex/native-dirplayer-readiness` at
  `/private/tmp/dirplayer-native-readiness-cumulative-20260918`
- Accepted cumulative source: `2812295899c5cd8e255f000a0df25f3dc78f1682`
- Accepted timer evidence parent: `a4aa876d26ecf535972912c28d26b09efcaa32c8`
- Combined-baseline source commit: `7cd3cc78eef4e5745472e41207690f6ce0874c9c`
- Combined-baseline source tree: `7ef95c925d97d7b90d930cd915996f9c2dbf098c`
- Source patch from the timer evidence parent:
  `3b4b188c7474b99a5783c8471c48da6ea9ff186b2da88ba29d55670507ab7c9f`

The combined-baseline commit changes only
`vm-rust/src/player/testing_browser.rs`: `await_child_command_idle` validates
the exact nested child owner, submits `DrainInputFlagCleanup` to that owner's
command queue with a `ManualFuture` completion, and awaits the FIFO barrier.
The nested fixture uses it after the first timer callback, the stale-incarnation
check, and the replacement callback. This preserves all timer/Flash assertions
while proving each preceding command reached its terminal turn.

## Verification

All Cargo commands used the existing shared target at
`/private/tmp/dirplayer-native-readiness-native-stage2-fff7dcc3/target`,
offline Cargo mode, and repository-managed tools through `mise`.

| Check | Result | Canonical log SHA-256 |
| --- | --- | --- |
| Full native library suite | 636 passed, 0 failed | `35282fe13cde2c68f37d82c7cbd01484d38db1f885da871006d6fd3fd3e11db2` |
| Native nested-fixture production parser integration | 1 passed, 102 filtered | `97e9100b82a4386ed505015973584010485a41b860a2a522754fa53df9656c86` |
| Production `wasm-pack build --target web` | passed, including `wasm-opt` | `3476af50fde61f41a4a6afb5a7de8164829cdae4ea26b2d1b62959f7be5438d7` |
| Generated-binding targeted TypeScript check | passed | `11f0ceafd5964ac8727b00910d55023ce20e298d7e2ee7f55a616d6c1246b826` |
| Production React/frontend build | passed | `153a2568edcf2e0e454c3c5669b74b0a4df4a01868c41ebde7e02e29d1bd45a4` |
| Combined production browser fixture gate | 22 passed, 0 failed | `9ae227cc2ce5f4cf1c3a17263d9e63250fe1624d325ed9e780373b1c43d131c3` |
| Flash manager tests, reused from the same unchanged TypeScript source | 18 passed | `f2b2abf29f393ef3554c575e35c77f66690abede6832068ee2ead60f803adcbb` |
| Browser-owner lifecycle tests, reused from the same unchanged lifecycle source | 38 passed | `7efdcbade049261eb8a2ea63a478e7b911cc432999323a55fccbbf7318655d5a` |

The two reused frontend receipts and hashes originate in
`docs/checkpoints/native-readiness-20260918/timer-runtime/README.md`; their
logs remain in that checkpoint's expanded timer evidence rather than being
duplicated in this baseline archive. The canonical frontend production log in
this checkpoint is `frontend-production-build-fixture-fix-rerun.log`, whose
hash is the `153a…` value above.

The combined browser filter covers the earlier 21 accepted BrowserOwner cases
plus `test_browser_owner_timer_lifecycle`. The 22 emitted cases include the two
generated browser-handle cases and the two host-event-guard cases, together
with nested input, public play, host-tail preservation, FileIO, SysMenu,
BudAPI, scripted Flash access, owned evaluator binding, JS-object bridging,
initial Flash access, nested Flash ownership, mouse dispatch/reentry, Lingo
callback, sprite variable ownership, Multiuser lifecycle, and timer lifecycle.

The failing pre-fix combined run was 21 passed/1 failed. An artifact-only
`test_nested_flash_owned_fixture` run reproduced the same failure in isolation,
ruling out cross-fixture leakage. Its log showed child A becoming Flash-ready
with one pending command. After the FIFO barrier change, the exact-source
combined run passed all 22 cases with one worker and a 180-second case timeout.

The green browser log still contains 31 nonfatal notifications that the browser
host event sink was not bound and one aborted request for a Ruffle WASM chunk
during instance teardown. Every selected assertion completed and the harness
reported 22 passed/0 failed, but these messages remain runtime-log noise to
watch during the native probe rather than being classified as fixed.

The production build was also attempted once with `CI=true`; React promoted
the existing Autoprefixer `start`/`flex-start` warning to an error. Repeating
the normal production command without that additional environment override
passed. No source change was needed for that tool-mode failure.

## Reproducible commands

From the frozen worktree:

```sh
CARGO_TARGET_DIR=/private/tmp/dirplayer-native-readiness-native-stage2-fff7dcc3/target \
CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0 \
mise exec -- cargo test --manifest-path vm-rust/Cargo.toml --lib --locked --offline

CARGO_TARGET_DIR=/private/tmp/dirplayer-native-readiness-native-stage2-fff7dcc3/target \
CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0 \
mise exec -- cargo test --manifest-path vm-rust/Cargo.toml --test mod \
  test_nested_flash_fixture_uses_production_director_parser --locked --offline

CARGO_TARGET_DIR=/private/tmp/dirplayer-native-readiness-native-stage2-fff7dcc3/target \
CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0 \
mise exec -- npm run build-vm

mise exec -- npx --no-install tsc --project \
  /private/tmp/dirplayer-native-readiness-baseline-20260918/generated-bindings-tsconfig.json

mise exec -- npm run build
```

The browser gate used `mise exec -- npm run e2e-test-browser --
--reporter=line --timeout=180000 --workers=1` with
`BROWSER_RUNNER_DIR=/private/tmp/dirplayer-native-readiness-baseline-20260918/browser_runner`,
port `9237`, `E2E_REUSE_SERVER=0`, and this filter:

```text
browser_player_handle,multiuser_socket_lifecycle,browser_handle_nested_input,fileio_open_remote,sysmenu,browser_player_host_tail_preservation,browser_handle_public_play,budapi,browser_handle_flash_scripted_access_owner_capabilities,browser_host_event_guard,flash_owned_evaluator_binding,js_object_owner_bridge,flash_initial_access_before_reservation,test_nested_flash_owned_fixture,test_flash_mouse_dispatch_decoder,test_owned_mouse_pointer,test_owned_mouse_reset_reentry,test_flash_lingo_callback_owned_fixture,test_flash_sprite_variable_owned_fixture,browser_owner_timer_lifecycle
```

## Artifact identity

- Generated WASM JavaScript: `badb697bda6249ae8f9058218a58e3bc28273dba12b24f16a9270f16fc3e0e66`
- Generated WASM declarations: `fd33719e3026d0afc0cca2ceb76ed24af03eac76c34d151eeddaf94949fb843e`
- Packaged production WASM: `949f2d19ebbaa9e9a7719ac027bfd8f7ccf29d7e2d4c4222d618789060786e90`
- Frontend-packaged WASM: `949f2d19ebbaa9e9a7719ac027bfd8f7ccf29d7e2d4c4222d618789060786e90`
- Frontend main bundle: `90fe0c979fc82ab4631fc71ceb9732df14c4e1ecb140fd62d08b7605f016da65`
- Browser-test WASM: `d12bf15c888993a81166e20239decc6ec7158c3274168f274c38379eb42177a6`
- Browser-test generated JavaScript: `63a4220adfdfd4d5c0bca9926e54acbe038679fda4e583258db12b65834ce741`
- Browser-test Flash manager bundle: `a78296eb40646854d8708148d9fea8c79ddfdb8d946cc88cdcafb8e6f32390ad`
- Browser-test API shim: `8b7eee1e39953abb12487cf7fb2b876b9e6f9069d34408fcec16d8c618929526`

The frontend contains exactly one `vm_rust_bg` WASM file, whose hash equals
the freshly generated package. Its source map resolves the VM binding as
`../vm-rust/pkg/vm_rust.js`; it does not resolve the stale package from the
original dirty checkout.

Expanded evidence is under
`/private/tmp/dirplayer-native-readiness-baseline-20260918`. The validation
archive is
`/private/tmp/dirplayer-native-readiness-baseline-evidence-20260918.tar.gz`,
SHA-256 `cadc800d2b1653f3b851a96cca62bf6fa7e4e67d278cb55e53ae76a80d397706`.

## Explicitly open checks

The 47 configured licensed/public movie paths are absent in this worktree, so
the full native licensed-movie campaign did not run. The exact inventory is
`validation/licensed-fixture-inventory.tsv` in the evidence archive, SHA-256
`0ead51455fe3cca5b927919e5c86a30ed7dd27b358aa08024a5a8adcc7dfb116`.
The checked-in nested Flash DCR fixtures are present and their production
Director parser integration passed.

`cargo check --tests --target wasm32-unknown-unknown` remains a known
configuration-specific outstanding check. The last exploration on the accepted
timer source reported 14 `E0599` errors in unit-test-only native helper calls.
That result is sourced from
`docs/checkpoints/native-readiness-20260918/timer-runtime/README.md` and its
external expanded timer log archive; it is not duplicated in this baseline
archive. This receipt does not mark that check passed and does not label it
pre-existing. The required production WASM build and browser-test WASM path
both pass.

The original dirty checkout is unchanged at
`fa80cb175625ac0e72022ff399247623f66b00cd`, with binary diff SHA-256
`15588ab0bdbda53db608b9b60bf06dde21bb55836b167f68fc4480bc3158593e`.
Its nested Ruffle checkout remains at
`79d1ca0f45d79e28c3a0658bbc6b8430d26ef308`, with binary diff SHA-256
`f928008992e7822afac077f74055b3a217ff424c78bf4400fb0aeafbcca5b11b`.

## Next obstacle

This source and artifact identity is safe to use for the disposable native
single-session probe. The next unknown is the first demonstrated unsupported
operation or host boundary on the native route. That probe is separate from
the preserved Stage 2/3 acceptance gates and from broader ownership cleanup.
