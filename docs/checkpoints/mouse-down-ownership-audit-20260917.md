# MouseDown command ownership audit

The child reset regression exposed a production command ownership gap after the
replacement child was explicitly put back into playback. The compiler-6 native
artifact executed
`player::nested::tests::direct_nested_reset_replaces_owner_and_both_runtime_channels`:
command completion succeeded, but the selected child's `movie.mouse_down`
remained false. The focused result was 0 passed, 1 failed (exit 101). This is an
open defect, not a passing child lifecycle checkpoint.

The path is `run_command_loop` -> `run_command_with_pending_pump` ->
`run_player_command` -> `run_player_command_result`. The last function checks the
captured session/player/owner, then its `MouseDown` branch calls ambient
`player_is_playing`, `reserve_player_ref`, `reserve_player_mut`, and legacy
script/Flash helpers. The initial owner check does not make those later
accesses owner-local. Exported `BrowserPlayerHandle` command loops use this same
path. Independent sessions can reuse player IDs, so adding `with_active_player`
cannot supply the missing session authority.

The bounded repair must retain the meaningful failing regression and migrate
MouseDown to the captured session and exact owner throughout. It must preserve
actor stepFrame ordering, scrollbar consumption, click/drag/focus state,
behavior/cast/frame/movie propagation, movie callbacks, and error-safe input
flag restoration. Existing owned handler-gap, event and evaluator helpers should
be reused. Flash pointer forwarding also needs an exact owner/instance-generation
route with short VM borrows and revalidation after host reentry; native unsupported
Flash effects must fail explicitly. No ambient adapter or no-op replacement test
is accepted as a fix.

Acceptance requires the direct-reset regression plus independent sessions with
colliding IDs, observable selected-owner state and script effects, stale-owner
rejection, and cleanup on error/reset. This is a Stage 2.4 input-command component;
other legacy command paths remain part of Stage 2.4/2.9, and the full child Flash
browser lifecycle gate remains separate and open.

## Review of the first local repair

The first uncompiled repair used one active input-scope identifier per player.
That is insufficient for overlapping guards: a second entry replaces the first
identifier while saving already-modified flags. Either drop order can then lose
the original snapshot and leave `in_mouse_command` set. Acceptance must exercise
both completion orders, deferred cleanup while the session is borrowed, reset,
and checked identifier exhaustion before mutation. Pending cleanup must drain
before another scope captures its snapshot.

Flash pointer forwarding crosses two host boundaries, first move and then down.
The captured Rust owner and instance generation must remain current between
those calls and after the second call. A single check after both calls cannot
prevent down from reaching a replacement created during move.

The event helpers also retain cleanup guards across awaits. In the reviewed
source, `OwnedEventStopScope` and `OwnedScoreContextScope` call `borrow_mut` in
their destructors. The real suspended-command cancellation path therefore needs
coverage under an existing session borrow; testing only the outer input-flag
guard cannot establish safe cleanup of the whole future. These are open review
findings, not accepted repairs or passing regression results.

## First combined native execution

Compiler-8 completed the locked offline native library test build successfully.
The wrapper subsequently failed because it assigned zsh's read-only `status`
variable; the retained Cargo result is separately recorded as exit 0.
The exact nested direct-reset regression now passes (1/0), as does the reset
exhaustion regression (1/0).

The seven focused MouseDown tests produced six passes and one failure. Passing
coverage includes colliding-session state isolation, stale-owner rejection,
both overlapping guard drop orders, deferred input-flag restoration under a
borrow, and identifier exhaustion. Reset cleanup still fails: clearing scope
records does not clear the replacement player's inherited `in_mouse_command`
flag. The successful reset path needs repair without changing failed-reset
preflight semantics. These results do not yet cover script effects, complete
suspended-command cancellation, or browser Flash pointer dispatch.

Local raw evidence: `/private/tmp/dirplayer-child-flash-validation/compiler-8/`
and `/private/tmp/dirplayer-child-flash-validation/native-8/`. This is an
intermediate failing checkpoint, not component acceptance.

Compiler-9 subsequently passed after the reset cleanup and typed Flash response
decoder repairs. All seven focused MouseDown tests now pass, together with the
direct nested reset and reset exhaustion tests (one test each). The frontend
manager suite passes 10 tests. Raw receipts are `compiler-9/`, `native-9/` and
`frontend-1/` under the same local evidence directory.

Whole-command cancellation remains open. The approved next regression enters a
real sprite MouseDown behavior, observes a pending evaluator callback, and drops
the command future while holding the session borrow. Static frame/movie fallback
needs equivalent coverage for its static-event guard. The bounded repair may
defer exact-owner event-stop/static-guard cleanup through session-owned records;
it must preserve overlapping scope restoration and reject stale reset cleanup.
An isolated input guard test does not satisfy this requirement.

## Combined native checkpoint

Native-12 executes all 595 library tests successfully, including 11 MouseDown
tests, 23 owned-event-scope tests and the four compiled-accounting tests. The
MouseDown tests now exercise actual behavior and static fallback suspension,
drop under a live session borrow, drained evaluator/guard cleanup, reset stale
cleanup, and a selected-owner script effect while another session with the same
player ID stays unchanged. The navigator inspected the retained test output.

The emitted binary SHA-256 is
`7300559ff611d174c494f10cd25dd61a9c4e88c7ecf84ea07c2bf31876721619`.
Raw output and critical source hashes are retained in
`/private/tmp/dirplayer-child-flash-validation/native-12/`; compiler-12 records
the frozen full source manifest. This proves the native checkpoint, not complete
input or child Flash acceptance. WASM-12 failed on two borrowed-capture lifetimes
in the browser fixture's `read_fixture` closure; the fixture repair, actual WASM
pointer-response coverage and embedded Ruffle browser gate remain pending.

The fixture lifetime repair subsequently passed WASM-13:
`cargo check --tests --target wasm32-unknown-unknown --locked --offline`
completed with exit 0. The repaired `testing_browser.rs` SHA-256 is
`acd76967028d8dd6d23a187b608df82e1e781eb04f28459ff133227e082e7b97`.
This is compilation evidence; pointer-response and embedded browser execution
still require their runtime checks.

## Interactive pointer fixture review

The staged fixture derives its button geometry from Ruffle's local
`from_shumway/button3/test.swf` (550 by 400 pixels, click at 250,200). It is
not yet integrated or runtime-verified. Source review found two issues to fix
before relying on it:

- AVM1 `DefineFunction` action length excludes the function body. Ruffle's
  `swf/src/avm1/write.rs` writes the header length then appends the body;
  `read.rs` adds `code_length` when consuming the action. The initial generator
  and its validator both included the body in that length, so the validator's
  passing result did not establish a valid executable fixture.
- The initial generator attached `onMouseMove` to a button. Ruffle's AVM1
  button event dispatcher does not handle that clip event; the move observer
  needs a MovieClip handler or a Mouse listener, while press remains on the
  real button.

The independent pilot is correcting the staged bytes and validator. Acceptance
still requires real embedded Ruffle to observe zero-valued initial globals,
move before press, and owner isolation. Reset between move and down must have
an external observable for absence of down; reading a retired owner's variable
cannot establish that result. Production Ruffle dispatch runs actions
synchronously, and its external-interface provider requires script access and
networking `All`. These are source-level feasibility findings, not runtime
reentry evidence.

The corrected staged fixtures now use header-only `DefineFunction` lengths, a
root MovieClip move handler, a button press handler and a terminal `Stop`. The
navigator inspected the generator/validator and independently regenerated both
fixtures in memory, compared them byte-for-byte and confirmed that the validator
rejects the old malformed encoding. Both files are 1900 bytes; SHA-256:

- A: `2112fdcd4153807408c02d3b72aeae08ca376787ca8a5a32aa98f5f6edf9c44d`
- B: `60c316ccea5c30301d6244d3123331b583bde6451498dedcd71897cff982f118`

This accepts structural staging only. The files remain under
`/private/tmp/dirplayer-pointer-fixture-staging/`; live browser integration and
the separate reset-reentry variant are still pending.

The three fixtures and portable generator/validator are now integrated under
`vm-rust/tests/fixtures/` as `flash_mouse_{a,b,reentry}.swf`,
`generate_flash_pointer_fixture.py`, `validate_flash_pointer_fixture.py` and
`flash_pointer_fixture_provenance.md`. The navigator repeated byte-for-byte
regeneration and negative checks from the repository. The reentry fixture is
2153 bytes, SHA-256
`26ecc422a5ad7d7ac7a392a58e4e9f61ef2f1ed67c9bb45b1d56dd27c030f268`.
Its negative test now mutates the actual `CallMethod` opcode, not the byte
`0x52` inside the callback name. Browser decoder/pointer/reentry tests are
approved for implementation; runtime verification remains outstanding.

## Resumed validation boundary

The resumed pointer fixture now registers cleanup before the fallible host call,
starts actual handle playback before input, and does both again after reset.
The release WASM prebuild is in progress; browser success is not yet claimed.

Read-only follow-up found that `dispatchMouseEventOnInstance` has a separate
MV3 bridge branch using fire-and-forget `bridgeCallMethod`. The current embedded
Ruffle gate exercises the direct synchronous branch. Its eventual result cannot
establish bridge delivery ordering, reset reentry or stale completion handling;
retain that transport in the Stage 2.5 extension audit. No bridge failure has
been reproduced and no bridge implementation change is included here.

## Accepted resumed component

The interrupted score/child-host/MouseDown component is now accepted at the
[source and artifact identity](score-child-pointer-20260917/acceptance.json):
600 native tests, four focused browser wrappers, the combined 19-fixture browser
gate, and final production WASM compilation pass. The
[acceptance record](score-child-pointer-20260917/README.md) states exact behavior,
fixtures and renderer boundaries. Earlier failures above are historical; full
Stage 2 and the separately listed remaining ownership paths stay open.

Read-only follow-up during callback work confirms remaining input consumers in
commands.rs: MouseUp still uses ambient handler-gap/draw/state helpers and must
preserve scrollbar-release swallowing, button hilite, original mouse-down target,
script ordering and saved input flags. MouseMove retains ambient drag state and
the existing 3D-only constraint semantics. KeyDown/KeyUp retain ambient key
dispatch and yield flags. These are future bounded ownership items; no migration
or runtime acceptance is claimed by this inspection.
