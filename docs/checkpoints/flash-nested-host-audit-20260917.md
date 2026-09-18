# Nested Flash host lifecycle audit — 2026-09-17

Read-only discovery against published runtime revision `7d9ca56df79e2027cd0d4f93ee67e93d50070776`, before the next initial-access implementation. This is a remaining-work record, not an acceptance receipt.

## Confirmed boundary

`RuntimeSession::register_nested_player` creates a child player, distinct owner, command/event channels and `NestedChildRecord`. `start_nested_movie_owned` starts both child loops before `load_movie_from_dir_owned` and `run_movie_init_owned`, and its startup guard invalidates a failed child owner and attempts registry removal. If the session is already borrowed during guard drop, registry cleanup is deferred to later teardown. No child browser Flash capability or frontend registration is established along this path.

Production `initVmCallbacks` captures one `FlashOwnerHost` created by `initFlashBridge(browserHandle)`. It registers VM callbacks under the root owner key. `dirplayer-js-api/index.js::onFlashMemberLoaded` first looks up that exact owner in `vmCallbacksByOwner`; unknown child owners are dropped. Even forwarding the callback to the root would fail the captured `flashHost.ownerKey` comparison and would grant the wrong authority if that comparison were removed.

`BrowserFlashCapability` already retains session, player ID, exact owner, binding state and command sender without owning player retirement. It is currently used by browser tests; its constructor borrows the session. Any production child export must therefore be constructed after the registration borrow ends, before child movie load/startup can emit Flash actions.

Host sprite keys and local channel numbers are different domains. Legacy nested keys use `NESTED_FLASH_BASE` and `NESTED_FLASH_STRIDE`. `checked_flash_sprite_number` accepts only positive `i16` channels, and `FlashBindingState` is local-channel based. `update_flash_frame_for_player` casts its incoming number to `i16`; sending a synthetic host key here would truncate it. Registering a child host alone is insufficient: generation, operation, pixel and callback routes need an explicit mapping, or the owned host must consistently use local channels while legacy compositor addressing is translated at its boundary. A nonzero root player ID must never be interpreted as a nested-child identity.

## Next implementation boundary

- Establish and retire a child host registration through an explicit browser capability lifecycle, with parent/child owner checks before and after host callbacks. Do not recover authority from ambient current-player state or numeric ID alone.
- Make child registration visible before movie load/startup actions. Failure or synchronous reentry must roll back only that child registration; a replacement owner must survive.
- Preserve local channels throughout owner-scoped capabilities. Document every required translation to legacy synthetic keys, including frame delivery and nested composition.
- Tie child teardown to failed startup, direct child removal, parent reset and parent disposal. Registrations and pending publication must not outlive the producing owner.
- Coordinate callback-origin owner/sprite/generation transport with Stage 2.7. Root initial-access tests do not establish child callback correctness.

Required acceptance includes two roots plus actual registered children sharing channel numbers, production embedded Ruffle load and first access, correct child pixel destination, delayed publication across parent reset, failed-startup teardown, and stale callback rejection after replacement. A test of synthetic-key arithmetic alone is not sufficient.

The root initial-access component may be accepted separately. Full Stage 2 completion still requires resolving this lifecycle gap and the remaining callback and rendering ownership gates.

## Active implementation contract

After the accepted root initial-access and evaluator-cancellation components,
the child lifecycle implementation remains local, unverified work excluded from this publication. This section records its design
and acceptance boundary, not a passing implementation result.

Owned child startup must publish an exact parent/child capability registration
after releasing the session borrow and before load or initialization emits Flash
work. Owned hosts use local channels; only the retained legacy nested path uses
synthetic host keys. Registration reentry must revalidate the same parent,
child owner and registry record before activating the child.

Reset must preserve behavior. A direct child reset rotates its owner, refreshes
the registry/host registration, and installs replacement command and event loops;
old loops capture retired authority. Parent reset continues to remove children
and permits later recreation. Failure/removal/disposal retire exact captured
registrations outside mutable session borrows. An old disposer cannot touch a
replacement child.

The browser fixture must exercise the production registration controller and
per-host callbacks, not a parallel harness implementation. Two roots and their
children share local channel numbers but use distinct authored values/pixels.
Acceptance requires real Ruffle first access, correct child frame destination,
continued operation after child reset, failed-startup cleanup, and stale
publication/callback rejection without affecting the other root. Instance-origin
callback work beyond this boundary remains the separate Stage 2.7 requirement.

## Retirement paths requiring final verification

The resumed review after published `3073bf6b` identified a separate callback
cleanup boundary in the local child implementation. Normal provider teardown
calls `FlashOwnerController.disposeNestedTree` before disposing its root host,
but `FlashOwnerHost.invalidateAfterCapabilityFailure` and direct host disposal
recurse through `disposeNestedFlashHosts`. That helper disposes child hosts
without removing the controller registrations or owner-qualified VM callback
tables. Host liveness checks alone do not prove callback-registration reclamation.

The child component must route every retirement path through exact subtree
callback cleanup, including capability failure after descendants are registered.
Verification should use the production controller and real VM registrar, inject
a failing captured capability, and demonstrate retired descendant callback
removal while an independent root and reentrant replacement survive. This is an
open review finding, not an accepted repair or passing test claim.

The direct-reset regression must also prove both replacement loops execute.
Command completion can use its existing completion future; event verification
must observe consumption or an effect without depending on a scheduler-specific
instantaneous queue length. A closed old queue can still contain pre-close work,
so retirement assertions drain it before checking its terminal closed state.

### Local retirement repair reviewed

The local repair now attaches both parent-subtree and exact-host retirement
hooks. An individual child's capability failure removes that child's callback
registration as well as its descendants; parent failure removes its subtree.
Hooks retain host identity and capability-scoped callback disposers, protecting
replacement registrations. The navigator reviewed the critical interfaces at:

- `src/services/flashPlayerManager.ts`: SHA-256 `fc694774a0df235d552be6ee4295fd3f735cf34b16bcdbc8e35b4665006c65eb`.
- `src/services/flashPlayerManager.test.ts`: SHA-256 `5a2a9305cc078addaf1ad1bb6a155bc9192054f11dd5106ac2b40579c0c163fe`.

The implementation pilot reports one passing suite with nine tests using
`CI=true mise exec -- npm test -- --watchAll=false --runInBand src/services/flashPlayerManager.test.ts`.
The lead supplied the terminal result, but no durable raw log was retained for
this run. Tests use the real VM callback registrar and cover a failed child and
grandchild, unaffected sibling and independent root, and a stale disposer after
replacement. This is bounded frontend evidence, not acceptance of the complete
child lifecycle. Production Director parsing subsequently passed its one named
integration test (see `nested-flash-fixture-20260917/`). Native reset execution
and the real two-root child Ruffle browser gate remain pending.

### Publication boundary

This documentation checkpoint publishes review findings and bounded historical
evidence only. Child Flash and MouseDown runtime changes, frontend changes,
fixture sources and browser tests remain unpublished working-tree changes.
The latest native direct-reset regression failed; the subsequent MouseDown
repair is incomplete. The last WASM compilation failed; its local mechanical
repairs have not yet passed a fresh combined build. No child lifecycle acceptance
or Stage 2 completion is claimed.

### Browser failure-cleanup probe reviewed

The revised local browser candidate exercises failure during synchronous flush
of a prepared action after the real VM callback registrar publishes the child
callback table. The injected callback throws, so nested startup must fail.
Afterward, direct production callback dispatch must not invoke that callback,
and the generation-aware Flash host route must return `unknown-owner`. The
other root must still read its authored value `9`.

Earlier assertions checked harness sets populated only after registration
succeeded. Their absence could not prove cleanup after failed registration;
those sets are now supplemental bookkeeping, not the failure-cleanup oracle.
The lead reviewed and froze `testing_browser.rs` at SHA-256
`8bff577f4dab34bbb1913307140b6a45a7e56ce6bfcff8fd0f88a73e8c47c490`
and the browser JS template at
`809df8e9e158bd727543abad07e961b46f2baf5d2ffdd83f5d40f82ab9b873a5`.
Syntax and whitespace checks passed. Actual WASM compilation and browser
execution of this candidate remain pending; this is source review only.

The subsequent frontend manager run passes 10 tests (local raw receipt:
`/private/tmp/dirplayer-child-flash-validation/frontend-1/`). The broader lifecycle
test initially reported 36 passes and two failures: the same-owner replacement
and pending-unload setups emitted loads before awaiting owner registration.
Both now await the real registration promise, matching production startup, and
the pilot reports 38 passes with `mise exec -- bun test
scripts/flash-owner-lifecycle.test.mjs`. The reviewed test-only change has SHA-256
`b332ac162aad2a0c3dea296115358bda1c3ad54599023b262022a025fa436c74`.
These frontend results do not replace the pending embedded Ruffle browser gate.

## First real child browser run

After native-12 passed all 595 tests and WASM-13 compiled successfully, the
focused `nested_flash_owned_fixture` browser run built its release WASM and
bundle, started a fresh server/Playwright runner, and entered the outer Playwright
test. Entry into the Rust fixture itself was not established. The
harness then remained pending for the 60-second test limit without publishing
a result, panic or script error. The run exited 1; it is not acceptance evidence.
The video and error context are retained locally under
`test-results/e2e-browser-e2e-tests/`.

The next diagnostic must identify the exact stalled await in owner registration,
child startup, Flash reads or reset. Bounded test phase logging is approved;
an unchanged retry or a larger timeout does not resolve the ownership/lifecycle
gate.

The diagnostic second run also timed out, with no `nested fixture:` markers
and no forwarded page console output. `log_test_action` writes directly to the
browser console, so the missing markers do not identify a stalled child await.
The next diagnostic must capture module/page errors, failed requests and initial
harness state, and verify console forwarding with a positive control. A module
startup failure must be distinguished from a runtime continuation stall before
changing production VM behavior. Raw second-run log:
`/private/tmp/dirplayer-child-flash-validation/browser-focused-2/run.log`.

Source-to-generated-module inspection found a concrete bootstrap defect:
`js_api.rs` imports `registerNestedFlashOwner` and `retireNestedFlashOwner` from
`dirplayer-js-api`, and the generated WASM JS statically imports those names.
The production API exports both, but the browser adapter template exports
neither; installing `window.dirplayer_*` hooks does not satisfy ES module named
imports. The approved repair re-exports the real production functions from the
adapter. It requires refreshing the generated adapter, not rebuilding Rust.
Browser diagnostics should confirm bootstrap and fixture entry after this fix
before drawing conclusions about child runtime awaits.

## Transport probe after bootstrap repair

The adapter now re-exports the real nested-owner functions. A fresh browser
transport probe using the existing WASM reached the Rust fixture: both root
registrations and child startups completed. Child A's first Flash read then
failed with `Flash request cast binding is stale or replaced`. This replaces
the unexplained timeout with a concrete runtime failure; it does not satisfy
the child lifecycle gate. No stale-cast repair was attempted before publication.
The child/input changes remain uncommitted work in progress.

## Score initialization ownership dependency

Current source inspection traces child initialization from
`run_movie_init_owned_at` through `begin_all_sprites` into
`Score::begin_sprites`. `RuntimeSession::with_player` passes an explicit player
and symbols but does not change ambient player selection. `begin_sprites`
still uses `reserve_player_*` for span eligibility, Stage member assignment
through `sprite_set_prop`, cast metadata and behavior allocation/attachment.
A correct parsed child score therefore does not establish correct mounted child
state. The native parser smoke test previously checked only score presence.

The next diagnostic mounts the real fixture through the production nested
startup with an entered parent channel 1, then compares the parsed channel/cast
pair and mounted child/parent state. If this confirms ambient score access as
the cause, the repair must pass the actual player through startup and its
helpers, preserving Stage and FilmLoop semantics with short borrows. It must
not introduce an active-player adapter or leave member assignment on an ambient
player after fixing only the eligibility check. No runtime repair or passing
regression is claimed in this source-level finding.

The native mounted-state regression now reproduces the defect (one failure,
595 filtered out, exit 101). The parsed frame 0/raw channel 6 pair is `(1,1)`.
The parent keeps member `(1,91)` and `entered=true`; child 2 has `member=None`
and `entered=true`, with owner session 79/player 2/generation 1 and
`FlashHostRoute::LocalOwned`. No Flash instance generation exists yet. The
navigator inspected actual stdout, failure diagnostics and source/artifact
receipts in `/private/tmp/dirplayer-child-flash-validation/native-mounted-diagnostic-1/`.

Artifact SHA-256:
`9b3604abae42e3c67bdebc2347fb3bfcffe82643868fe6f8e59b706c2581c601`.
`nested.rs` source SHA-256:
`bac3e7fa3b51827cc5937a46bca8745fba8ddc75c2bedde45b8003545560b93c`.
This new failure supplements the earlier passing 595-test checkpoint; the
current augmented test suite is not green. The production migration proposal
is pending, and the cast-binding fence remains unchanged.

## Accepted resumed component

The interrupted score/child-host/MouseDown component is now accepted at the
[source and artifact identity](score-child-pointer-20260917/acceptance.json):
600 native tests, four focused browser wrappers, the combined 19-fixture browser
gate, and final production WASM compilation pass. The
[acceptance record](score-child-pointer-20260917/README.md) states exact behavior,
fixtures and renderer boundaries. Earlier failures above are historical; full
Stage 2 and the separately listed remaining ownership paths stay open.
