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
