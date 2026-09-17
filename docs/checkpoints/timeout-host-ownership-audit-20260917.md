# Timeout host ownership follow-up

Read-only audit after the accepted evaluator-cancellation component. No timer
repair or runtime failure reproduction is claimed here.

`Timeout::schedule` and `Timeout::cancel` dispatch only a name and period through
`JsApi`. `dirplayer-js-api` forwards these notifications to ambient `vmCallbacks`.
In `src/vm/callbacks.ts`, the interval closure captures that registration's root
`browserHandle`, and handles are stored globally in Redux by timeout name alone.
The explicit datum handler still invokes `Timeout::schedule` while operating on
the caller's player. Its checked datum access does not qualify this host effect.

Consequently, the current transport lacks the information needed to distinguish
two players scheduling the same name, and its selected callback registration can
belong to another root. Cancelling one name also lacks an owner discriminator.
This is a source-level ownership gap for Stage 2.3/2.4/2.9, separate from the
accepted retained-timeout datum and evaluator cancellation work.

A bounded repair must qualify schedule/clear/fire with the captured owner and
timer incarnation, retain interval handles in an owner-local host registration,
and perform host work after the mutable VM borrow ends. Reset, disposal and
same-name replacement must retire only their captured interval. Preserve dormant
period-zero behavior, exact-case keys and the existing case-insensitive lookup
fallback; replacing these semantics would not satisfy ownership migration.

Required evidence: two exported browser handles scheduling identical names,
independent firing/clear, replacement with a delayed old tick, reset/disposal,
period-zero activation and callback reentry. Include actual child timer delivery
once child host lifecycle exists. Native behavior requires its own explicit
scheduler/unsupported-effect accounting; the no-op JS stub is not proof.

## Proposed implementation boundary

The source review traces schedule/clear through `Timeout::schedule/cancel`,
`JsApi`, ambient JS callbacks, Redux name-only interval storage, and finally
`BrowserPlayerHandle::trigger_timeout` / `PlayerVMCommand` carrying only a name.
A delayed interval tick therefore has no timer-incarnation capability to compare
before looking up a same-name replacement.

The proposed component adds checked per-player timer incarnations and detached
owner-qualified schedule/clear actions, drained outside the mutable session
borrow. An owner-local frontend controller retains intervals; tick delivery
carries the captured owner and incarnation through the command queue and rejects
stale ticks before firing. Reset and disposal retire old-owner intervals before
callback rebind. Exact-name lookup, the case-insensitive fallback and dormant
period-zero behavior remain part of acceptance.

This is a design proposal, not an implemented or tested fix. Integration must
serialize with the active child Flash work in shared command/reset/host-drain
files. Native action tests cannot substitute for exported-browser-handle routing
and callback-reentry fixtures.

The navigator additionally inspected the manual due-timer pump
`fire_pending_timeouts_owned_at` and the legacy `TimeoutTriggered` command. The
manual pump snapshots target/handler/name before awaiting each handler and then
rechecks only the player owner; a preceding handler can replace or forget a
later same-owner timer. Incarnation validation must cover this retained ready
list as well as JS ticks. The browser command currently uses ambient accessors
and dispatches the stored handler with its existing object/non-object argument
rules; its owned replacement must preserve those rules. Manual test-pump timing
and system-timeout semantics must not be silently substituted for browser firing.
