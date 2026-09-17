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
