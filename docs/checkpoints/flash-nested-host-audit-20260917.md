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
