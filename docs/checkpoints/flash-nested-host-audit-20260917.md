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
