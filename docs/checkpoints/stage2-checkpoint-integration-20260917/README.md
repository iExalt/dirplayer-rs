# Stage 2 combined-checkpoint integration

This receipt records the bounded integration of the retained combined review
checkpoint into `dev` over Stage 1 commit
`c0eb16f1c9dcabb5fcb43d6e5b5e413711fd8b25`. It does not accept all of Stage 2
or any later stage.

The archived [combined patch and manifest](../native-ownership-20260917/README.md)
contained 37 paths. Thirty-six runtime and test paths were applied at their
recorded `after` hashes. The `mise.toml` hunk was excluded because it removed
`CARGO_INCREMENTAL=0`; the live Stage 1 file remains at
`a8d10548ed3a7fbe794a851d546c3367af6f1618e617769f726868c6b086e4e4`
and retains the Stage 1 tasks. The Ruffle submodule remains at
`79d1ca0f45d79e28c3a0658bbc6b8430d26ef308`, with all seven pre-existing
dirty-file hashes unchanged.

The [runtime source manifest](runtime-source-manifest.tsv) covers 516 runtime
inputs, including the seven retained Ruffle files. Its SHA-256 is
`a637a29ae8aadf0e797c0afeb5ccc5d6a46edac67745cfa1de3c6459d9cc9c92`.
The same manifest was captured immediately after integration and after all
verification; the comparison had zero mismatches. `git diff --check` also
passed.

## Verification

All commands ran from the live checkout with the shared target
`/private/tmp/dirplayer-parser-review-build/target` and
`CARGO_INCREMENTAL=0`.

- Native test-target compilation passed with `--locked --offline`. The exact
  `vm_rust` library executable selected from the fresh Cargo JSON artifact
  metadata was `vm_rust-6c8ff607dfc83df9`, SHA-256
  `fc4fec696f4341f49401409753e2f5388ea26dc414807eed12e51d857e04cca4`.
  Direct execution passed **480 tests, 0 failed**.
- WASM all-test compilation passed for `wasm32-unknown-unknown` with
  `--locked --offline`.
- `mise exec -- bun test scripts/flash-owner-lifecycle.test.mjs` passed
  **34 tests, 0 failed**.
- The first browser attempt regenerated the runner, then stopped before tests
  because the sandbox denied binding `127.0.0.1:9237` with `EPERM`. The same
  command was rerun outside the sandbox and passed the exact **13 fixtures,
  0 failed** filter recorded below.
- The targeted live TypeScript check passed with zero diagnostics against
  `src/services/flashPlayerManager.ts` and the freshly generated declaration
  `mod-2671e42b945ed1b1.d.ts`, SHA-256
  `85dc45c4884484b93b89725a0ad7330f3b3669672d0485d2bc1f5c161606203e`.

Native compilation:

```sh
env CARGO_INCREMENTAL=0 CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/private/tmp/dirplayer-parser-review-build/target mise exec -- cargo test --manifest-path vm-rust/Cargo.toml --tests --no-run --locked --offline --message-format=json --target-dir /private/tmp/dirplayer-parser-review-build/target
```

WASM compilation:

```sh
env CARGO_INCREMENTAL=0 CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/private/tmp/dirplayer-parser-review-build/target mise exec -- cargo check --manifest-path vm-rust/Cargo.toml --tests --target wasm32-unknown-unknown --locked --offline --message-format=json --target-dir /private/tmp/dirplayer-parser-review-build/target
```

Browser runtime:

```sh
env CARGO_INCREMENTAL=0 CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/private/tmp/dirplayer-parser-review-build/target CI=1 E2E_FILTER='browser_player_handle,multiuser_socket_lifecycle,browser_handle_nested_input,fileio_open_remote,sysmenu,browser_player_host_tail_preservation,browser_handle_public_play,budapi,browser_handle_flash_scripted_access_owner_capabilities,browser_host_event_guard,flash_owned_evaluator_binding' E2E_REUSE_SERVER=0 E2E_VIDEO=retain-on-failure BROWSER_RUNNER_PORT=9237 E2E_CONSOLE=1 BROWSER_RUNNER_DIR=/private/tmp/dirplayer-parser-review-build/target/browser_runner mise exec -- npm run e2e-test-browser -- --reporter=line --timeout=60000 --workers=1
```

Targeted TypeScript:

```sh
mise exec -- npx --no-install tsc --project /private/tmp/dirplayer-stage2-integration-evidence/tsconfig-live-flash.json
```

The raw local logs and artifact metadata are under
`/private/tmp/dirplayer-stage2-integration-evidence/`. The durable outcome and
hashes are also summarized in [verification-summary.json](verification-summary.json).

## Boundary

This checkpoint integrates owner and generation aware Flash routes, owned host
events and notifications, lifecycle tests, and the associated browser API
changes. It does not integrate the separate Ruffle callback-origin or JS-Lingo
proposals. Residual player, renderer, bitmap-reference and other ownership work
remains. The complete licensed movie campaign is not verified; the broader
Habbo regression still lacks its required fixture. Passing the native library
suite does not establish a working native player or independent-session
acceptance.
