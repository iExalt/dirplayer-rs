# Proposed next component: callback-origin delivery

Preparation only. Finish and accept the current score/child-host/MouseDown
component before assigning this item. The required Sol lead must review the
proposal and coordinate Luna pilots; no source edits are authorized by this
note alone.

## Boundary

Move the production sprite `setCallback` registration and actual embedded
Ruffle callback delivery onto the captured owner, local sprite and instance
generation through VM dequeue. Preserve existing target-selection and argument
semantics, including LocalConnection bookkeeping. This is Stage 2.7; do not
absorb unrelated extension, renderer, or interpreter cutovers.

## Source findings

- `player/handlers/datum_handlers/sprite.rs` registers directly through the
  free `dirplayer_ruffleRegisterLingoCallback` window export while inside
  `runtime.with_player_and_symbols`. Moving only the frontend wrapper misses
  this real entrypoint and its borrow boundary.
- `src/services/flashPlayerManager.ts` installs a global delivery closure that
  selects its captured `browserHandle`; origin metadata is absent from that ABI.
- `BrowserFlashCapability::trigger_lingo_callback_on_script` enqueues raw data,
  but the command currently contains no originating sprite/generation fence.
  Validation must occur before dequeue-time argument decoding and allocation.
- The nine-file preserved Ruffle proposal removes the shared callback registry
  in favor of per-player registries and snapshot-before-host-call delivery.
  Its before-hashes matched live files during resumed read-only preparation.
  Recheck before applying; preserve the seven existing Ruffle edits.

## Required acceptance design

Use an authored/licensed fixture that invokes the actual registered Ruffle
callback. The pointer ExternalInterface reset hook is not this callback path.
Exercise overlapping local sprite IDs and identical callback names across
owners, successful delivery to each original owner, replacement/reset/disposal,
and a queued old-generation payload rejected at VM dequeue. Retain a positive
control showing that the real callback fires; absence alone is insufficient.

Capture immutable metadata at registration, preserve it through frontend
transport, and revalidate it at delivery/dequeue. Do not attach current metadata
to old callbacks or retain mutable VM/registry borrows across host reentry.
Malformed, stale and native-unsupported effects need explicit outcomes.

## Integration planning

The lead should propose exclusive Rust/frontend and Ruffle file ownership,
registration request/response ABI, and an exact verification selection before
editing. Existing owned Flash request/continuation patterns should be reused.
One monitor owns each build; keep targets, generated evidence and logs under
`~/Projects`. Compile the actual Ruffle artifact and run the embedded browser
fixture plus relevant existing regressions before root acceptance. A patch,
registry unit tests, or mocked frontend transport alone cannot close the gate.
