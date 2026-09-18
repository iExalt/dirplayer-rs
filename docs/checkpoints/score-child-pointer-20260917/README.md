# Accepted score startup, child Flash host and MouseDown component

Navigator acceptance applies to the working-tree source identity in
[acceptance.json](acceptance.json), over runtime `fa80cb17`. It does not close
the remaining Stage 2 ownership work. Nothing in this checkpoint was committed
or pushed as part of resumed implementation.

The selected player's score now owns startup member assignment, Stage property
effects and FilmLoop behavior construction. Native coverage preserves the
initial-load/script-edit flag semantics, relative FilmLoop cast references and
behavior identity through active-score startup. The prepareMovie-related test
simulates its score phase; it does not execute a prepareMovie script.

Child Flash startup registers and retires the exact owner, uses local channels,
and cleans partial registration failures. MouseDown retains owner/generation
through move/down host calls and rejects reset reentry. Shared session-ID
allocation prevents browser handles and the harness from producing identical
host keys. Reset clears loaded/ready/queued Flash state so retained members
reload under the new owner. Readiness retries only the typed prepublication
missing-instance state while keeping the expected generation.

## Accepted checks

- Full native library: 600 passed, 0 failed.
- Focused regenerated Director parsing, child mount/unsupported-host rollback,
  and reset/requeue contracts: one passing test each.
- Four focused browser wrappers: 4 passed, 0 failed; Playwright 33.6 seconds.
- Combined previous 15 fixtures plus those four: 19 passed, 0 failed;
  Playwright 45.2 seconds.
- Final locked/offline production WASM release: passed, 1 minute 40 seconds.
- Whitespace check: clean.

Navigator inspected the completed native/browser/build results and verified
the final key source and both WASM hashes. The manifest records 47 source,
configuration and fixture hashes plus eight receipt hashes. Production WASM
and the browser-loaded test module are distinct artifacts.

Raw receipts remain under
`/Users/clliaw/Projects/.dirplayer-work/native-stage2/validation/`; their exact
paths and hashes are in the manifest. The final production receipt is preserved
as `wasm-release-reset-final.log`. Earlier uses of `wasm-release-final.log` in
the chronological resume note are superseded by this unambiguous final receipt.

## Browser reproduction

Run from the repository with the recorded source/toolchain and existing Ruffle
assets. Both runs used `mise exec -- npm run e2e-test-browser -- --reporter=line
--timeout=120000 --workers=1`, with:

```sh
CARGO_INCREMENTAL=0
CARGO_NET_OFFLINE=true
CARGO_TARGET_DIR=/Users/clliaw/Projects/.dirplayer-work/native-stage2/target
MISE_CACHE_DIR=/Users/clliaw/Projects/.dirplayer-work/mise-cache
CI=1
PLAYWRIGHT_BROWSERS_PATH=/Users/clliaw/Projects/dirplayer-rs/.cache/native-bevy/playwright
E2E_REUSE_SERVER=0
E2E_VIDEO=retain-on-failure
E2E_CONSOLE=1
BROWSER_RUNNER_DIR=/Users/clliaw/Projects/.dirplayer-work/native-stage2/target/browser_runner
```

The focused run used port 9240 and:

```text
test_nested_flash_owned_fixture,test_flash_mouse_dispatch_decoder,test_owned_mouse_pointer,test_owned_mouse_reset_reentry
```

The combined run used port 9241 and this exact `E2E_FILTER`:

```text
browser_player_handle,multiuser_socket_lifecycle,browser_handle_nested_input,fileio_open_remote,sysmenu,browser_player_host_tail_preservation,browser_handle_public_play,budapi,browser_handle_flash_scripted_access_owner_capabilities,browser_host_event_guard,flash_owned_evaluator_binding,js_object_owner_bridge,flash_initial_access_before_reservation,test_nested_flash_owned_fixture,test_flash_mouse_dispatch_decoder,test_owned_mouse_pointer,test_owned_mouse_reset_reentry
```

## Coverage boundaries and next work

The nested rendering gate observes real authored Ruffle pixels through child
CPU rendering, the parent's bitmap and the parent's actual WebGL2 canvas.
Pointer fixtures use Canvas2D. Canvas2D linked-Movie composition has no rendering
arm and is outside this component's verified coverage. MV3 pointer transport
and the remaining input/extension/service ownership cutovers remain open.

Earlier failing runs exposed fixture geometry, transparent-background-only
content, malformed shape encoding, missing template bindings, a session-ID
collision and reset cache retention. They are superseded by the final checks,
not reclassified as successful evidence. Their diagnoses are retained in the
[resume history](../reboot-handoff-20260917/resume.md).

The next assigned component is Stage 2.7 callback-origin ownership: real
Ruffle registration, frontend delivery and VM dequeue must preserve captured
owner/sprite/generation. Its preserved nine-file patch remains unintegrated
until the new architecture proposal is reviewed.
