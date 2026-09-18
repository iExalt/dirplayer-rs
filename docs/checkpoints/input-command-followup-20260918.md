# Remaining input command ownership

Read-only preparation for Stage 2.4; no implementation or runtime acceptance.
The sprite-variable component is accepted. The active timer component retains
priority and exclusive ownership of shared runtime files. Assign keyboard work
separately after timer acceptance and explicit release of those files.

Current `BrowserPlayerHandle` methods in `vm-rust/src/lib.rs` update keyboard and
pointer state through `with_context` before queueing commands. Key up/down and
mouse movement also route to nested players. This immediate input state update
is distinct from queued script delivery and must remain available while a script
yields inside a key-pressed loop.

The corresponding commands in `player/commands.rs` still use ambient access:

- MouseMove checks ambient playback, mutates pointer/drag state and calls the
  ambient rollover dispatcher. Preserve field-scrollbar drag precedence,
  moveable-sprite offsets and the existing Shockwave3D-only constraint behavior.
- RightMouseDown/RightMouseUp select the top scripted sprite and otherwise invoke
  frame/movie handlers. Preserve the existing targeting and right-button state;
  these paths do not perform left-button highlighting or drag selection.
- KeyDown/KeyUp wait through the ambient handler-gap helper and hold drawing
  through ambient player state. KeyDown toggles `command_handler_yielding` around
  an awaited handler. Migration must restore that state on error/cancellation
  without touching a replacement owner. An owned handler-gap helper already
  exists in `player/mod.rs`; reuse its identity checks rather than introducing
another selected-player adapter.

`player/keyboard_events.rs` confirms that queued handlers deliberately do not
update `keyboard_manager` again: replaying key-down state after browser key-up
would revive a released key. Both wrappers establish a fresh `stopEvent` scope;
focused sprite behavior and editable-member handling also need preservation.
`events::dispatch_rollover_events` updates the hovered set and dispatches leave
before enter/within. Its deferred event delivery must capture the originating
owner, not merely calculate the hovered set through an explicit context.

Split keyboard delivery from mouse movement/right-button delivery unless source
discovery proves a common indivisible boundary. MouseUp still needs its own
closure review; accepted MouseDown evidence does not cover it. Do not infer
ownership closure from the exported handle's initial `with_context` call.

For either assignment, require actual exported two-handle browser tests with
overlapping local sprite IDs, nested delivery where the public API routes it,
reset/disposal during waits, a surviving neighbor, and handler reentry. Keyboard
acceptance must exercise a yielding key-pressed loop released by immediate key-up
state and must preserve existing handler/result semantics. Pointer acceptance
must cover script targeting, drag state and rollover delivery across owners.
Native tests should cover captured-owner state/error cleanup; they cannot replace
the exported browser entrypoint evidence. Keep host calls and waits outside any
mutable VM borrow.
