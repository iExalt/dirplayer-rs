# Resume after reboot

## Latest stopping point — user-requested stop, 2026-09-18

The user requested: "Finish what was currently in flight and stop." Work is
stopped. Do not resume implementation or validation without a new user request.
Stage 2 remains authorized as the eventual scope, but is incomplete and paused.

Timer lead and pilot have stopped. All known exec handles completed; ports 9101
and 9244 were closed. The navigator's outside-sandbox process check found no
matching task Cargo, wasm-pack, Playwright, webpack or browser-runner processes.
No commit, push, revert or follow-on build was performed for this stop.

Current timer checkpoint:
- Earlier focused Rust 15/0, full native 635/0, frontend manager 18/0 and lifecycle
  38/0 passed before the subsequent TimeoutRef evaluator repair.
- The TimeoutRef/TimeoutInstance internal property executor repair is in tree;
  its focused native eval/drain regression passes 1/0 at
  `~/Projects/.dirplayer-work/timeout-owner-stage2/validation/native-eval-period-drain.log`.
- Earlier WASM check, production package and frontend builds passed, but are
  stale after later Rust changes and diagnostic instrumentation.
- Browser rerun2 passed the previous hang and reached synchronous reset. Timer
  retirement succeeds, but the assertion expecting two exact Clear calls fails.
  Receipt: `~/Projects/.dirplayer-work/timeout-owner-stage2/validation/browser-owner-timer-focused-rerun2.log`.
- In-flight test-only instrumentation landed in the browser template, runner
  exports and Rust fixture. It records `during-reset` versus `post-reset` Clear
  phases. Diff-check passes, but this instrumentation has NOT been executed.

Important reconciliation requirement: the pilot ran rustfmt on module-root
files, recursively formatting many unrelated Rust modules. These edits are
preserved, not accepted. Separate formatting from functional changes against
the preserved pre-timer/accepted source identities before acceptance or packaging;
do not bulk-revert files that also contain accepted or in-flight changes.

On an explicitly requested resume: reestablish Sol-lead/Luna-pilot ownership,
reconcile formatter spillover, then run the focused browser fixture to identify
the missing Clear phase. Do not weaken the cleanup contract without evidence.
Final WASM/package/frontend, combined browser and source/artifact acceptance are
still pending. Keyboard and all other remaining Stage 2 items remain unassigned.

## Earlier resume history

The user explicitly authorized resumption after reboot. This supersedes the
pause instruction in README.md while preserving that historical stopping point.
All 43 dirty/untracked source identities in worktree-manifest.json matched when
resumption began. Both repository branches/remotes remain as recorded.

The old temporary Cargo target no longer exists. Ongoing work, build caches,
validation output and handoffs must stay in the repositories or ~/Projects.
Use /tmp only for truly disposable scratch files. Current paths:

- CARGO_TARGET_DIR=/Users/clliaw/Projects/.dirplayer-work/native-stage2/target
- Logs/evidence: /Users/clliaw/Projects/.dirplayer-work/native-stage2/validation

Updated subagent-pair-program skill requires a Sol lead for every item and
explicit Luna model/effort selection. Current lead is /root/ownership_lead,
gpt-5.6-sol/high, with two gpt-5.6-luna/high pilots and exclusive core/pointer
file sets from the handoff. The lead owns all Cargo/browser runs, routine
proposal approvals, repairs and first-pass reviews. The main agent owns final
acceptance and remaining Stage 2 scope. No commits or pushes authorized now.

First acceptance sequence: finish interrupted score/pointer source; review;
freeze sources; reproduce and fix the mounted child regression; verify
independent-owner, Stage side-effect and FilmLoop semantics; compile WASM;
execute actual child and pointer/reentry browser tests, then combined gates.
Historical passing 595-test evidence does not prove the interrupted migration.

## Native Flash diagnostic boundary correction

On resumption, source review established that fixing first-pass member mounting
will let run_movie_init_owned reach FlashHostAction::Load. Its native emitter
intentionally returns `Flash host is unavailable on native`, and startup must
reject/clean up instead of leaving a live child. The original failing diagnostic
expected a live child with no generation only because missing member setup
prevented host work. Preserve that historical failure receipt, but correct new
acceptance tests: prove parsed/mounted selected-owner member state before host
work, then prove full native Flash startup returns its explicit unsupported
error and cleans the child without mutating its parent. Successful full startup
requires actual WASM/Ruffle browser evidence. Never suppress native unsupported
effects to make a diagnostic assertion pass. The lead owns this test repair.

## First resumed compile and independent pointer review

The first cold native compile populated the Projects-local Cargo target, then
failed with an unclosed delimiter in the score migration. Its receipt is
`/Users/clliaw/Projects/.dirplayer-work/native-stage2/validation/compile-first-pass.log`.
The lead owns ordinary compiler repairs and subsequent validation.

Navigator review also identified two pointer-fixture setup defects, returned
to the lead for repair: registration cleanup guards were installed only after
the fallible registration await, and the fixture never transitioned the
selected player into playing state before MouseDown. Reset stops playback, so
recreation needs that transition too. These findings are not passing browser
evidence; real browser execution remains required.

Warm compile three reached two remaining mutable-borrow errors in sound-trigger
bookkeeping (`compile-third-pass.log`). The core pilot reports both repaired;
that report has not yet been accepted by compilation. Pointer setup now calls
the production handle `play()` method. Its registration guard still requires
arming before the external registrar invocation to cover publication followed
by a synchronous throw; arming only after the call covers Promise rejection
but leaves that synchronous failure uncovered.

The lead encountered transient `Selected model is at capacity` failures while
resuming its turn. Keep the required Sol lead role; do not bypass it or silently
switch models. Pilot files are currently idle pending lead review/validation.

The lead subsequently resumed successfully. The fourth warm native compile
passed (`cargo test --tests --no-run --locked --offline`, incremental disabled;
`compile-fourth-pass.log`, exit 0). Runtime tests and WASM/browser acceptance
remain pending; the pointer guard correction continues through the lead.

The pointer guard ordering is corrected at source hash
`9588567bc03cc6ad0cf980fe71b8d94f51c5578df866758054f096e52e1561d2`.
The first focused native contract passes 1/1 (`native-mounted-contract.log`):
selected child 2 mounts member 1:1 with entered=false, properties applied and
no script write; parent remains member 1:91/entered. Full native startup then
returns the exact unsupported Flash host error and removes the child.
Remaining regression packets are Stage/prepareMovie, then FilmLoop, followed
by full native, WASM and actual child/pointer browser gates. This bounded pass
does not establish full component or Stage 2 acceptance.

The focused score gate then passed 2/2 (`native-score-begin.log`, 596 filtered):
Stage intrinsic member sizing, initial-load flags, playing-state second pass,
both one-flag negative cases, puppet/name semantics, and FilmLoop local-score
assignment/relative behavior construction/repeated attachment count. The mounted
contract re-passed with an explicit one-player graph assertion after cleanup.
The lead froze all six owned source files and started the release WASM test
prebuild. Full native suite and actual browser gates remain pending.
Navigator suggested strengthening the FilmLoop fixture to compare the retained
behavior identity and invoke the actual begin_all_sprites active-score pass;
that test-only follow-up must not mutate files during the frozen WASM build.

Release WASM prebuild passed (`wasm-release-prebuild.log`, 3m10s). Afterward,
the FilmLoop test was strengthened to retain the exact behavior id/owner through
begin_all_sprites. The final native library suite passes **598/0**
(`native-lib-full-final.log`, no filtered tests, 0.32s execution). The lead is
refreshing the warmed release WASM artifact for the final source identity, then
running all four focused browser wrappers and the prior combined regression
selection augmented with those wrappers. Full component acceptance is pending
those actual browser results.

Read-only preparation for the later Stage 2.7 callback component: all nine
entries in `../native-ownership-20260917/callback-before-hashes.json` still match
the live Ruffle files. The preserved callback patch remains unapplied and
uncompiled. Matching its baseline permits review without reconstructing lost
source; it does not establish design acceptance, transport integration or
runtime correctness. Complete the current browser gate before assigning that
separate component to the lead.

The final-source release WASM link passed (`wasm-release-final.log`, 2m57s).
Focused browser execution then reached all four Rust wrappers
(`browser-focused-four.log`): decoder passed; two child sessions each used
numeric child id 2 and returned isolated Flash values A=7/B=9. The nested test
then failed because its pixel helper assumed a 2D renderer context. Pointer
tests lacked a created renderer before play and raced instance readiness,
producing missing-instance/zero-counter failures. The lead is repairing those
fixture setup boundaries using a real handle canvas, actual owner/generation
readiness and real parent composition observation. No production-score repair
is indicated by this run; full browser lifecycle acceptance remains open.

Independent follow-up found both manual fixture installers also omit active
score spans (and do not puppet their channel). Production get_sorted_channels,
get_sprite_at and parent composition therefore exclude those channels. The lead
is batching valid score/frame setup with renderer/readiness repairs before the
next browser build. Use the actual score/input/render paths; do not replace them
with direct child pixel reads or direct Flash input dispatch. Any explicitly
chosen Canvas2D backend must be named as that test's coverage boundary.

After renderer/span setup and generation-latched readiness, the second focused
browser run reports 1 pass and 3 failures (`browser-focused-four-second.log`): decoder passes,
both child instances reach ready and isolated reads 7/9, but parent Canvas2D frame
image creation fails; pointer readiness sees the pre-publication host response
without a generation. Navigator traced the image precondition to the default 0x0
parent movie.rect: draw_frame sizes its bitmap from movie.rect, even though
renderer canvas layout clamps dimensions. Set authored parent movie rectangles
(nested 32x32, pointer 550x400) before creating renderers. The lead is also routing
a genuine, owner/generation-qualified publication wait before invoking readiness.
Do not suppress malformed readiness responses or adopt a replacement generation.

The lead traced a legitimate protocol state rather than just a fixture delay:
the owner-qualified TS readiness route returns typed missing-instance before
publication, without a generation. The navigator approved a bounded production
repair in wait_for_flash_ready_owned: inspect ok/code first; only missing-instance
continues the existing bounded wait at the captured owner/expected generation;
successful responses must contain that same safe generation. Unknown/disposed,
stale/invalid and other errors fail, with owner checks across host work and waits.
Native unsupported behavior remains explicit. The pointer pilot now owns this
helper change in flash_object.rs as well as browser fixture repairs. Require
fresh affected validation and actual prepublication-to-ready browser evidence;
previous compilation receipts do not verify this new production delta.

The readiness repair passed fresh native 598/0 and WASM release; the third
focused browser run still has one pass and three failures
(`browser-focused-four-third.log`). Child reads remain isolated and parent
Canvas2D drawing now executes, but composition precedes captured-frame delivery.
Pointer readiness retries prepublication correctly yet times out without a
Ruffle creation marker. The lead is diagnosing publication and adding an exact
child frame-delivery precondition for composition.

Navigator found a concrete namespace collision candidate: HarnessRuntime's
TEST_SESSION_ID and lib.rs NEXT_BROWSER_SESSION are separate counters starting
at 1, and both construct player 1/generation 1. Distinct sessions can therefore
serialize to the same host owner key. Verify actual harness/handle keys and
prepared-action routing before further fixture changes. The lead must propose
a shared checked allocation path rather than test offsets or clearing owner
retirement tombstones. This is source evidence, not yet a proven runtime cause.

The lead confirmed both allocation paths can produce the same serialized key.
Navigator approved a shared checked next_runtime_session_id allocator in
player/session.rs, migrating both lib.rs browser allocations and
testing_shared/mod.rs HarnessRuntime::new. Core pilot owns those files; pointer
pilot retains its fixture/helper files. Remove the independent counters, keep
constructor exhaustion explicit, and test exhaustion with a local atomic rather
than any global reset hook. Browser assertions must distinguish the harness and
both explicit handles' full owner keys while retaining child-local id collision.
Fresh native/WASM/browser verification must establish the repair; do not clear
tombstones or choose arbitrary test identity offsets.

The shared allocator implementation uses zero only as a permanent exhaustion
sentinel after returning u64::MAX once. Navigator reviewed the three migrated
consumers and local-atomic exhaustion test; no allocated zero escapes. Fresh
validation remains pending. Navigator also returned a browser-helper review:
wait_for_nested_flash_frame must retain its first/captured generation across
polls, rather than re-reading and validating a potentially replaced generation.
The lead will finish the active run, repair that test helper, then validate the
final source. Do not duplicate the lead's build or process monitoring.

Further read-only preparation for 2.7 confirms that sprite.rs setCallback calls
the free global Ruffle registration export directly, while the frontend's
global delivery closure selects its installed browserHandle. Consequently the
future component must migrate the production registration call, frontend
transport and raw VM command together with the preserved Ruffle patch. Validate
captured origin metadata again at VM dequeue, before decoding/allocating args;
delivery-time current-owner stamping is insufficient. This is a next-item design
boundary, not authorization to start that item before the current gate closes.

The shared allocator passed fresh native 599/0 (`native-lib-shared-session-final.log`),
focused allocator 1/1 (`native-session-allocator.log`) and release WASM
(`wasm-release-shared-session-final.log`, 1m42s). Navigator inspected these completed
receipts. Lead reports WASM SHA256
`52bd9014c417d73b938a641b33c4aff228290c12f65c324b6fe1c5f69d6b3e9b`.
The fourth browser run (`browser-focused-four-fourth.log`) confirms distinct
harness/handle owner keys and actual Ruffle publication/readiness; decoder passes.
Three checks fail: black parent composition, zero pointer effects, and zero
reset-reentry positive-control press. Native/WASM success does not close them.

Lead diagnosis found the pointer fixture has loc=(0,0) with centered 550x400
registration, producing rect (-275,-200)-(275,200). The injected (250,200) lies
on the excluded bottom edge. Repair the authored Director loc to (275,200) and
assert actual selected-owner playing/hit state before real command dispatch.
This fits the existing fixture scope; no production pointer repair is yet
indicated by that failure. Parent composition diagnosis remains independent.

Navigator traced a concrete composition fixture defect: the generator emits
SetBackgroundColor and DoAction but no shape, production sets wmode=transparent,
and Ruffle Player::render submits transparent RGBA zero in that mode instead of
the stage background. The lead is repairing the fixture with actual colored
shape content and placement, preserving production transparency and requiring
real parent composition. The exact-generation child-frame precondition may
inspect frame content; it must not wait for BitmapId replacement because frame
updates replace pixels in the existing bitmap. The pointer loc/hit assertions
and generation latch were frozen in a WASM relink before this source diagnosis;
reuse its compilation evidence only where applicable, then build the repaired
fixture once and run the required actual browser gates.

The first shape repair's structural pass was invalidated by navigator review
against Ruffle's SWF parser: StateFillStyle1 omitted its index bit, placement
flags 0x03 incorrectly requested replacement without a matrix (use 0x06), and
SWF character/depth IDs used the Director big-endian helper. The validator
mirrored those errors. Also, the retained original SWF RECT describes a 1x1
pixel stage, not the intended 32x32 stage. The lead cancelled the just-started
browser runtime and assigned one fixture-only repair packet covering all four
issues and independent field decoding. That run's test-harness compilation
passed, but it supplies no accepted runtime result. Final fixture hashes must
be regenerated; real Ruffle parsing/rendering and parent pixels remain required.

Production vm_rust.wasm and browser `--test mod` are separate artifacts. Remaining
fixture/helper-only edits do not justify rebuilding unchanged production WASM.
Record the browser-loaded test WASM identity in the final acceptance packet.

Corrected fixtures reached real browser execution (`browser-focused-four-final2.log`):
decoder and reset-reentry pass. Initial A/B pointer move/press ordering also
passes within the still-failing pointer wrapper; its later reset/recreation
wait sees no remounted generation. Lead is diagnosing the real reset boundary
before proposing production edits. Do not weaken that recreation requirement.

Nested expected-color child RGBA now arrives under the captured generation,
but parent Canvas2D pixels remain black. Source review confirms Canvas2D's
render_stage_to_bitmap lacks a Movie arm, while WebGL2 explicitly consumes
nested_movie_images. Navigator approved switching only nested parents to the
production WebGL2 backend and synchronous post-draw readPixels (correct GL Y
origin). Acceptance remains child render -> parent bitmap -> real parent pixels;
no production renderer changes. Canvas2D linked-Movie composition remains an
unsupported/unverified backend boundary for later rendering work.

The lead corrected its initial reset hypothesis: `entered` is not the cause.
reset_core tears down host instances and rotates flash_binding_state but retains
flash_sprite_loaded, so pre-dispatch refuses to queue the preserved member's
new load. Navigator approved clearing flash_sprite_loaded, flash_host_actions,
and flash_ready_sprites at that teardown boundary in mod.rs; clarify that
sticky readiness applies within one owner across member swaps, not across
reset. Core pilot/lead owns this bounded production repair. Preserve score
content and neighbor state. Require native proof that old actions/caches are
cleared and a preserved member queues a current new-owner Load, then actual
browser recreation and combined gates. Numeric instance generation may restart
at 1 under the distinct owner; do not require globally distinct generations.
No change to Score::reset entered semantics is authorized by this diagnosis.

Final-source focused native reset requeue, regenerated mounted/unsupported-host,
and production Director-parser tests pass (one each):
`native-reset-flash-requeue-final.log`, `native-mounted-regenerated-fixture.log`,
and `native-parser-regenerated-fixture.log`. The final3 browser run has 3/4
passing wrappers: decoder, pointer two-owner/reset-recreation/readiness rejection,
and reset-reentry. Nested actual WebGL2 parent red/blue pixels, child reset,
stale rejection and sibling isolation pass before a missing window helper
binding stops the remaining lifecycle checks. Navigator approved adding the
eight already-exported helpers to tests/browser_templates/index.template.html.
Reuse the compiled test artifact and rerun packaging/runtime; full four-wrapper
and combined-19 acceptance remain pending. Full native 600/0 is not yet recorded.

The subsequent final4 run passes all four wrappers (Playwright 1/0, 33.6s):
`browser-focused-four-final4.log`. Loaded browser test WASM SHA256 reported by
lead: `aa1fd5cad03db6f1ff8b871f3bfdaea657b187e92b3bf908a0cf658e92549b1c`.
Fresh full native passes 600/0 (`native-lib-reset-final.log`, 0 filtered).
Navigator inspected both finished result receipts. The lead is now running the
exact prior combined selection augmented with the four wrappers (expected 19,
120s timeout), then refreshing production WASM for the reset change. Component
acceptance is pending those checks and the final source/artifact manifest.

## Current accepted boundary and next assignment

The navigator accepted the score/child-host/MouseDown component after native
600/0, browser focused4/0 and combined19/0, final production WASM release,
critical-interface review and source/artifact hash verification. See
[acceptance](../score-child-pointer-20260917/README.md) and its 47-path manifest.
The final production receipt is preserved as wasm-release-reset-final.log.
The Sol lead now has a new bounded Stage 2.7 callback-origin assignment: return
an architecture/ABI and exclusive file-ownership proposal before source edits.
The previous nine-file Ruffle proposal is still unapplied. Root owns status
documents; lead owns discovery/pilot coordination. Full Stage 2 remains active,
with no Stage 3 work and no new commits/pushes.

Stage 2.7 architecture is now approved for implementation. The core Luna pilot
owns VM/frontend/harness changes; the pointer Luna pilot owns the preserved
nine-file Ruffle patch and its tests. Both remain gpt-5.6-luna/high under the
gpt-5.6-sol/high lead. setCallback always uses the pending request boundary,
captures owner/local sprite/generation, and releases VM borrows before host
registration. A stable owner-table callback route preserves that metadata into
the raw VM command, which validates its captured OwnerToken and instance
generation before decoding/allocating arguments. Preserve existing cast-target
selection and LocalConnection bookkeeping; no separate LC redesign.

Navigator required an explicit true registration acknowledgement from the
receiver-bound Ruffle TS method after successful WASM registration. The generic
synchronous extension bridge otherwise returns null/undefined for both missing
methods/replies and successful void calls. Direct and bridge registration must
reject missing acknowledgement and retain post-call owner/generation checks.
The lead owns routine pilot review and all builds; it will report Ruffle cache
feasibility/build estimates before the cold build. Full callback acceptance
still requires actual AVM1 callback invocation, owner collisions, stale queued
payload rejection, reset/disposal and surviving-sibling regressions.

Stage 2.7 Ruffle core gate reports 113/0 including four registry tests. Web
build first failed at tsx IPC (`listen EPERM`) inside the sandbox; the identical
pinned build is now running outside it. Dedicated durable paths:
`~/Projects/.dirplayer-work/stage2-7/ruffle-target` and sibling `validation`.
Existing Node modules/dist remain available; no Ruffle Cargo cache survived.
The lead estimates cold native/web build work at roughly 20–40 minutes and owns
monitoring. Actual web artifact/browser callback verification remain pending.

Navigator identified a bridge-delivery gap in the approved registration design:
Ruffle emits the callback extern in MV3's main world, while the stable host table
is in the isolated world. public/dirplayer-ruffle-bridge-host.js currently has an
owned LocalConnection forwarder but no Lingo-callback forwarder. The lead must
propose the minimal callback-only transport carrying the original owner/sprite/
generation before expanding edits to that host/client pair. Do not acknowledge
bridge registration successfully while leaving its emitted callback unresolved.
Transport tests must be distinguished from actual extension qualification.

The minimal bridge expansion is approved: a main-world callback forwarder
emits exact captured origin fields; one shared isolated-world listener invokes
the same stable owner-table route as direct delivery. Add bridge-host.js and
ruffleBridgeClient.ts to the VM/frontend pilot's ownership. Require exactly-once
delivery across two owners and disposal of A without removing B's listener.

Ruffle core 113/0 and actual web/selfhosted build pass. The lead copied the new
bundle into public/ruffle. Navigator inspected the finished receipts and verified
all seven copied files against `stage2-7/validation/public-ruffle-artifacts.sha256`.
Key JS hash: `5cc8363e28fecc3f6a92dbcb1a8fed1ee3beefdc543dd5a59ac283ccf3ca01f9`;
WASM: `d31a754ec1bfa7477cbb8a1e67fa1342ad1d7eeb9b835ce55352e5af76dca122`.
This is build evidence only; callback runtime acceptance is still open.

Lead first-pass review returned two VM registration repairs: use the new owned
ABI instead of the removed free registrar, and restore LocalConnection map
insertion to validated preparation before the host call. Raw dequeue fencing
is present before raw_flash_args. The fixture must register through actual
Lingo sprite.setCallback, including a ready-cache hit, and queue a valid-origin
callback before replacement to prove later pre-decode stale rejection. Direct
host registration and an already-stale synthetic command alone are insufficient.

Callback follow-up: navigator review required latching generation before host
emission, embedded FlashObject owner/sprite/generation validation, metadata and
LC preparation before readiness, and short-arity no-op preservation. It also
found a last-installed-host direct callback closure that the lead replaced with
one shared owner-table route for direct and DOM delivery. A separate Ruffle
receiver now queues untouched base64-array JSON with an explicit encoding; the
VM decodes only after its dequeue origin fence. Public PlainJson and LC remain
unchanged. Navigator critical review found no further blocker in these repaired
interfaces; runtime acceptance is pending.

The completed Ruffle pilot now owns authored SWF generator/validator/provenance
files, while the core pilot owns VM/frontend and Rust/browser wrappers. Host
CallFunction bypasses the registry hook, so the fixture has an authored wrapper
using ActionCallMethod to invoke the registered probe. It records count and
Unicode/numeric/object arguments. Direct callback hooks are not runtime proof.

Intermediate checks: C1/C2 native compilation passed, decoder 3/3, frontend
ownership 12/12, full native 601/601. Additional focused native registration
tests and initial string-path browser registration are being added. The first
C2 WASM release attempt failed with 26 new testing_browser.rs helper errors
(nested Results, temporary borrows, moved Symbol); that helper is WASM-only and
part of the library artifact. The core pilot is repairing this one batch before
rebuild. Failed receipt is stage2-7/validation/c2-production-wasm-release.log.
Lead remains sole build monitor; no production artifact/browser callback pass
yet. Latest concise status: ../callback-origin-20260917/README.md. No commits or
pushes in this resumed work; full Stage 2 remains active.

Later callback review required recursive retained-object binding: callback
marker conversion now carries captured owner/sprite/generation after dequeue
validation; LC remains unchanged. C3 native passes 607/607 and WASM release
passes, but real browser execution exposed allocation failure before the later
RefCell panic. Explicit diagnostic-only source/artifact counters prove repeated
admission of the same queued EvalId serial1 without first poll; work reaches128,
inflight remains empty, and individual futures are only ~10KiB. A class cap was
rejected because nested same-class requests are required. A biased-select trial
also failed and must be reverted. The lead now has approval for synchronous
identity admission before boxing: take Eval request or mark/requeue exact action
ticket started, then execute captured work via extracted helpers. Preserve
session cancellation and nested concurrency. Add unpolled-admission regression,
then run diagnostic callback gate and remove all diagnostic counters/abort before
final artifacts. Full receipts/current status remain in callback-origin README.

Identity admission regression is green and now has exact-ticket validation
before first poll, including same-owner caller cancellation. Browser work stays
bounded but initial registration still looped. Corrected capped diagnostics show
exact EvalId1/EvalAction2 ordinary Object requeued unchanged. Approved dispatch
repair is now in source under lead first-pass review: Unsupported datum calls
reuse classify_async_object; remove reason-only Object retention; return explicit
EvalRequestTurn::SpriteAsync only after attached-script precedence lookup; both
drivers execute it outside VM borrow and share typed resume validation. Focused
native full-evaluator unsupported-host and attached-script precedence tests are
next, then a marker-verified capped browser run. Other unsupported Object fallback
can still requeue and remains Stage2.2 work. Diagnostics are still temporary;
none of the later callback/scheduler work is accepted yet.

Latest resumed checkpoint (2026-09-18): focused native callFunction preparation/
unsupported-host checks pass 2/2, and C11 actual browser now executes the authored
wrapper and inner AVM1 call (both counts 1). The callback still does not publish
its retained target in Lingo. Lead ownership_lead is tracing registered callback
emission and delivery; do not restart the already resolved fixture investigation.
Named root-timeline functions, dotted invocation path and a valid JSON-array
argument string are now in the fixture. SpriteAsync callFunction has a dedicated
exact-owner/generation path outside the VM borrow; its legacy args[1] string and
String-or-Void behavior are preserved. The neighboring get/set legacy routes
remain open. C11 runtime receipt is
`stage2-7/validation/c11-browser-callback-owned-call.log`. Complete chronology is
in callback-origin README. Diagnostics still require removal before rebuilding
final native/WASM/browser acceptance artifacts. No new commit or push occurred.

Current acceptance checkpoint (supersedes earlier in-progress callback notes):
Stage 2.7 is accepted locally at callback-origin-20260917/acceptance.json, with
612 native, 12 manager, 38 lifecycle/LocalConnection and 20 actual browser tests
passing plus matching production package/frontend artifacts. Actual reset and
handle destruction with sibling count5 are verified. All diagnostics removed.
The navigator independently checked source/evidence/artifact hashes and retained
61 source/config/fixture files in the accepted-source-overlay.tar.gz under
~/Projects/.dirplayer-work/stage2-7. Full Stage 2 remains active; no Stage 3 and
no new commit/push. The Sol lead's renewed assignment is sprite getVariable/setVariable
ownership. Discovery is complete and implementation is approved: explicit
SpriteAsync paths, atomic prepublication binding transitions preserving absent/
unresolved early handles, detached exact-generation host work and a separate
strict owned sprite bridge getter. Core pilot owns Rust state/score/dispatch;
frontend pilot owns bridge/manager code and tests. Production packets require
lead review before browser fixture integration. See
sprite-variable-ownership-audit-20260918.md for the exact behavior, cleanup
authority and acceptance boundary.

2026-09-18 successor acceptance: sprite getVariable/setVariable is now accepted
locally at `../sprite-variable-20260918/acceptance.json`. Native 628/0, frontend
15+38, focused browser 1/0 and combined browser 21/0 pass, including the corrected
early cast-0:0 property/method and stale-replacement assertions. Production VM
package and frontend build pass; navigator verified 62 source identities,
15 artifacts and 19 evidence receipts. The accepted 62-file overlay is under
`~/Projects/.dirplayer-work/sprite-variable-stage2/accepted-source-overlay.tar.gz`.
No commit or push. Full Stage 2 remains active; do not start Stage 3.

The prior Sol lead is releasing shared files/build ownership. Next implementation
is the reviewed timeout contract in `../timeout-host-ownership-audit-20260917.md`:
exact owner/incarnation, detached schedule/clear, root/child host registration,
stale tick/manual-ready rejection and native unsupported accounting. Timer
discovery is complete with Sol/Luna review. Optional sound-channel discovery is
paused because its required pilot could not acquire an agent slot; it made no
edits and returned no proposal. Root owns acceptance/status documents.

The sprite lead confirmed all process handles exited and port 9244 closed.
`/root/timer_discovery_lead` now has the renewed implementation assignment with
one Luna-high pilot, sole build ownership and evidence under
`~/Projects/.dirplayer-work/timeout-owner-stage2`. Read the reviewed timer
contract before editing. The lead must approve the pilot's concrete approach;
root retains cross-component decisions and final acceptance. No timer gate is
accepted yet.

## Scope clarification pause (2026-09-18)

Resolved by the user's explicit instruction to proceed through Stage 2 and stop
before Stage 3. The navigator reread the updated subagent-pair-program skill and
renewed the timer lead's bounded assignment. The pause below is historical.

The visible user request says to stop after Stage 1, while the saved active goal
and prior handoff say Stage 2. The navigator has asked the user which boundary
to follow. Automatic goal continuations do not answer that pending question.
Do not resume implementation until the user resolves it; preserve all work.

The timer lead confirmed a safe stop with no running build or test process.
Its reported checkpoint is 15 focused Rust timeout tests, 18 frontend manager
tests, 38 lifecycle tests, and 635 full native tests passing. These are partial
receipts, not timer acceptance. The wasm32 check failed in the browser fixture;
receipt: `~/Projects/.dirplayer-work/timeout-owner-stage2/validation/wasm-check.log`.
The pilot fixed two borrow-order errors. The remaining unimplemented fixture
repair would transfer root_b into JavaScript after its ordinary reset proof,
retaining root_a for disposal, so the reentry hook calls the real handle reset.
After scope confirmation, the lead must review that repair and complete WASM,
production package/frontend, real-browser, and final source/artifact checks.
No files were reverted or deleted; no new commit or push occurred.

After explicit Stage 2 resumption, the timer lead reports production WASM
packaging and frontend build passing. The focused actual-browser test reached
execution but rejected two fixture expressions using unsupported command-eval
syntax. A bounded syntax repair is with the pilot; focused and combined browser
validation plus the final source/artifact audit remain pending. This is progress
evidence, not timer acceptance. The lead retains sole runtime/build ownership.
