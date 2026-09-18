# Timeout host ownership follow-up

## Current checkpoint (2026-09-18)

Implementation and lead review are in progress; the original audit below is
historical. The lead reports 15 focused Rust, 635 full native, 18 frontend manager
and 38 lifecycle tests passing before the latest browser-fixture corrections.
Production WASM packaging and frontend build subsequently passed. The focused
browser test reached execution but rejected fixture command syntax; a bounded
syntax repair was followed by a browser timeout exposing a production eval-pump
gap: internal TimeoutRef SetProperty had no executor/completion producer. The
lead has assigned an exact TimeoutRef/TimeoutInstance execution path and native
eval/drain regression to the pilot. Affected production/browser evidence must be
refreshed after that repair. No timer acceptance is claimed.
Final acceptance requires the reviewed contract below, actual root and child
timer behavior, regression coverage and matching final sources/artifacts.

The user explicitly authorized completing Stage 2 after the scope clarification.
Sol lead `/root/timer_discovery_lead` retains sole runtime/build ownership with
its Luna pilot. Durable receipts remain under
`~/Projects/.dirplayer-work/timeout-owner-stage2`.

## Original source audit

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

## Reviewed implementation contract, 2026-09-18

Sol-high discovery lead and Luna-high source pilot independently traced the
production routes without edits or builds. Navigator approved the design below.
After sprite-variable acceptance released shared files, the lead received the
implementation assignment and its Luna-high pilot is actively implementing.
No timer verification gate or final acceptance is claimed yet. Evidence belongs
under `~/Projects/.dirplayer-work/timeout-owner-stage2`; the lead is the sole
build owner and root owns status/acceptance documents.

- Make TimeoutManager the sole mutation authority. Every create/reschedule,
  including dormant period-zero replacement, gets a checked JS-safe incarnation.
  Allocate before destructive mutation; exhaustion preserves the existing timer
  and queued actions. Preserve exact keys and exact-first/case-insensitive
  fallback lookup. Host commands always carry the canonical exact key.
- Queue ordered owner/name/incarnation-qualified Schedule and Clear actions.
  Drain outside the mutable session borrow. Schedule validates the current owner
  and exact scheduled incarnation before emission and across reentry. Clear is
  idempotent retired authority and may run after its owner retires; it cannot
  affect a replacement interval. Replacement orders Clear before Schedule.
- Reset collects old-owner retirement actions before owner rotation. Removal
  retires timers before dropping the player. Do not lose detached cleanup when
  an owner is no longer live.
- Browser TimeoutTriggered commands validate owner, incarnation and scheduled
  state before allocating values or dispatching. Preserve object targets receiving
  `[timeoutRef]`, non-object targets receiving `[target, timeoutRef]` through the
  stored global handler, and Void targets receiving `[timeoutRef]`.
- Manual due-timer snapshots retain exact name and incarnation, and revalidate
  before each callback after earlier callbacks may mutate later entries. Keep
  one sampled now and `next_fire_ms = now + period`. System-event fan-out remains
  a separate path using its requested system handler and arguments.
- Generalize BrowserFlashCapability and the nested registration/controller into
  an exact-owner browser host capability/registration supporting Flash and timers.
  Root and child registrations use the same callback factory. Preserve Flash
  behavior and remove obsolete internal names/call sites coherently; do not add
  a temporary Flash-only timer method or a name-only routing fallback.
- Each owner registration owns its timer controller with exact-name entries and
  incarnation-qualified handles. Reset/disposal clears only that registration.
  Remove Redux's global timeoutHandles storage and migrate its consumers.
- Native detached browser-timer effects report a typed unsupported outcome;
  the existing explicit manual pump remains the internal scheduler. Silent
  native JS stubs are not acceptable evidence.

Required focused tests cover ordered replacement, period-zero activation,
atomic incarnation exhaustion, case-distinct siblings and fallback cancellation,
stale queued Schedule/tick/manual-ready entries, argument rules, system-event
semantics, and native unsupported accounting. Actual browser fixtures must prove
same-name root isolation, reentrant replacement/clear/reset, disposal, and real
nested-child delivery and retirement. A test-only hook may invoke a captured old
tick; production routing must stay unchanged. Existing Flash/child/callback
regressions must remain green after registration generalization.

Likely ownership groups are timer model/datum handler; shared session/commands/
reset/removal/JS ABI; frontend registration/controller/Redux; and browser fixtures.
The implementing lead must serialize shared-file changes and retain one build
owner. This contract is design evidence only, not a completed Stage 2 gate.

First implementation review: the pilot returned a draft without running builds.
The Sol lead found remaining Flash-specific capability/controller ownership,
missing timer retirement through RuntimeSession::remove_player and only mocked
browser timer coverage. The same pilot received a bounded correction covering
general owner registration, removal retirement, atomic reservation, stale queued
ticks/reentry, allocator edge cases and real two-root/nested browser fixtures.
No compilation, test pass or timer acceptance is recorded at this checkpoint.

A later coordination check found two unanswered pilot checkpoint requests.
Before extending the wait, the lead inspected authoritative source progress:
recent edits introduce BrowserOwnerCapability/Host/controller/callback factory
and nested BrowserOwner ABI across Rust, TypeScript, JS API and browser template;
RuntimeSession queues pending timeout host actions during removal, and browser
fixture source has changed. The scoped old Flash-specific names are gone. These
are advancing edits, not reviewed completeness or runtime proof; browser runner
exposure and full lifecycle coverage still require the lead's handoff review.
No build/test result is claimed.

Second review narrowed the residual: the generic owner ABI and removal queue are
present, but the new root lifecycle fixture was not exported/registered in the
browser runner, and root/nested fixtures fabricated JS schedule/clear instead
of creating timers through the production Rust path. The controller also needs
replacement publication before reentrant clear and a setInterval-return reentry
case. The pilot is correcting those seams. Browser acceptance must create and
mutate timers through owned Rust/Lingo and its normal detached-action drain;
test hooks may replay captured old ticks, not manufacture the registration being
tested. No passing compilation or runtime gate is recorded yet.

Third review found that owned Lingo evaluation queues timeout actions without
draining them at its boundary, and the nested fixture probes unrelated arithmetic
instead of an observable timer handler. The lead assigned production-path drain
integration, actual child handler/stale-tick assertions and synchronous Rust
Schedule reentry coverage. Draining only when the outer handler future finishes
is insufficient: actions created before a yielding handler must reach the host
at the normal safe turn/suspension boundary, outside mutable VM borrows. Controller
replacement ordering and browser export wiring passed this source review; no
runtime acceptance is implied.

First focused Cargo attempt reached compilation and failed before tests with
five mechanical Rust errors: a nested Result at the suspension return, an
incorrect test wrapper return expectation, two string-reference mismatches and
a partial move in a receipt pattern. The lead assigned one compiler-local repair
batch. Receipt: `~/Projects/.dirplayer-work/timeout-owner-stage2/timeout-focused.log`.
The initial zsh wrapper also used the read-only `status` variable; subsequent
runs must use `test_rc` and capture the actual command exit. This failed attempt
is not a passing native gate.
