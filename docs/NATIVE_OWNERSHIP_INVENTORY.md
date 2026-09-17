# Native DirPlayer ownership inventory

Status: Stage 1 inventory for the `native-bevy` branch.

This document records mutable runtime state found in the browser-oriented
DirPlayer implementation and assigns each item a target owner for the native
session model. It is an inventory and migration contract; it does not make the
runtime changes itself.

The target model has three explicit identities:

- `SessionId` identifies one independently configured runtime session.
- `PlayerId` identifies the host player or a nested `#movie` player within that
  session.
- A generation/epoch identifies one lifetime of a player after reset. External
  work and retained handles must carry the identity they were created under.

VM values are owner-qualified. A VM value from another session or player is
rejected before any arena slot, counter, bitmap, or script instance is read.
Individual VM objects remain thread-affine; independent sessions communicate
through owned commands and results.

## Rust runtime state

| Current symbol | Current storage | Target owner | Classification and migration requirement |
| --- | --- | --- | --- |
| `player::PLAYER_OPT` (`mod.rs:7287-7289`) | `static mut Option<DirPlayer>` | `RuntimeSession::players` | Runtime singleton. Remove the ambient host player and expose an explicit player context. |
| `ACTIVE_PLAYER_ID` (`mod.rs:4264`) | `static mut usize` | Execution context / scheduler item | Runtime singleton. No current-player variable, TLS equivalent, or task wrapper may replace it. |
| `NESTED_PLAYERS`, `NESTED_PLAYER_KEYS`, `NESTED_EVENT_TX` (`mod.rs:4266-4276`) | Parallel `static mut Vec<Option<...>>` registries | Session-owned player graph and per-player scheduler endpoints | Runtime singleton. Use typed `PlayerId` slots and owner-qualified nested-player references. |
| `PLAYER_TX`, `PLAYER_EVENT_TX` (`mod.rs:7287-7288`) | `static mut` channel senders | Owning player/session scheduler | Runtime singleton. Commands and events must carry player and generation identity. |
| `PLAYER_GENERATION` (`mod.rs:7292-7295`) | `static mut u64` | Player lifetime metadata | Runtime singleton. Reset advances the owner epoch and stale messages are discarded before execution. |
| `reserve_player_ref`, `reserve_player_mut`, `player_ref`, `player_mut`, `active_player_ptr` (`mod.rs:4328-4404`) | Unsafe ambient accessors | `ExecutionContext<'_>` with explicit `player()` / `player_mut()` | Runtime access path, not an owner. Remove after call sites accept context. |
| `WithActivePlayer`, `with_active_player`, `spawn_player_local` (`mod.rs:4406-4487`) | Future wrapper around `ACTIVE_PLAYER_ID` | Session-owned scheduler with explicit owner-bearing tasks | Runtime workaround. Delete rather than converting it to another ambient mechanism. |
| `SYMBOL_TABLE` and `SYMBOL_TABLE_INIT` (`symbols/symbol_table.rs:27,144-217`) | Process-global mutable `SymbolTable` guarded by `Once` | One interner per `RuntimeSession` | Runtime singleton. Builtin IDs may remain process-stable immutable data; dynamic symbols must carry session identity. |
| `Symbol::{as_str,into_str,as_lower_str}` (`symbols/symbol.rs:38-134`) | Looks up global table and returns `&'static str` | Owned strings or borrows tied to an explicit session/context | Lifetime and ownership hazard. No dynamic symbol may return a process-lifetime string borrowed from another session. |
| `ALLOCATOR_RESETTING` (`allocator.rs:15-19`) | `static mut bool` consulted by `Drop` | Owner token epoch/reset state | Runtime singleton. Reset invalidates the epoch before arena teardown. |
| `DatumRef::Drop` (`datum_ref.rs:65-111`) | Raw count pointer plus active-player allocator lookup | Owner token with immutable identity/epoch and owner-local deferred-drop queue | Runtime ownership violation. It must validate owner and epoch before arena access; it must never dereference a foreign or stale arena entry. |
| `ScriptInstanceRef::Drop` (`script_ref.rs:38-63`) | Raw count pointer plus active-player allocator lookup | Same owner token/lifecycle queue as `DatumRef` | Runtime ownership violation. Foreign and stale references are rejected before counter access. |
| `DatumAllocator` arenas and counters (`allocator.rs:249-684`) | Embedded in each `DirPlayer`, but refs do not retain its owner | Player-owned allocator with owner token and explicit queue drain | Per-player state with unsafe cross-owner reclamation. Keep mutable arena access borrowed; refs retain lifecycle metadata only. |
| Bitmap effects from `on_datum_ref_dropped` (`allocator.rs:227-238,448-470`) | Reclamation returns a bitmap ID to the ambient player's manager | Owner-local queue entry drained while borrowing that player's allocator and bitmap manager | Must preserve recursive drop behavior and ephemeral bitmap counts without storing a pointer to the VM. |
| `js_lingo_loader::{JS_RUNTIMES,JS_OBJECTS,NEXT_JS_OBJECT_ID,CURRENT_RUNTIME}` (`js_lingo_loader.rs:35-51`) | Thread-local maps, ID counter, and runtime stack | Session-owned JavaScript runtime registry and explicit call stack | Runtime TLS. IDs and object handles require session/player/generation qualification. |
| `bytecode::handler_manager::EXPRESSION_TRACKER` and `EXECUTION_HISTORY` (`bytecode/handler_manager.rs:21-49`) | Thread-local execution state | Explicit interpreter execution state owned by a player invocation | Runtime TLS. A thread can host interleaved sessions only through explicit state. |
| `js_api::{SCORE_DIRTY,PENDING_CAST_LISTS}` (`js_api.rs:261-265`) | Thread-local dirty state | Player/session notification state | Runtime TLS. Callback messages carry owner identity. |
| `rendering::{RENDERER_LOCK,LAST_DRAW_MS,DRAW_LOOP_SPAWNED,WEBGL_CONTEXT_LOST}` (`rendering.rs:3649-3661`) | Thread-local renderer and loop state | Session-owned presentation/render service | Runtime TLS. The native Bevy frontend owns presentation; VM workers do not use ambient renderer state. |
| `bitmap::{DEFAULT_SYSTEM_PALETTE,DST_QUANTIZERS}` (`bitmap/bitmap.rs:123-1438`) | Thread-local mutable preference/cache | Session-owned palette preference and quantizer cache | Runtime TLS. Immutable palette tables remain global. |
| `font::GLYPH_PREFERENCE` (`font/mod.rs:38-40`) | Thread-local preference | Session-owned font service/configuration | Runtime TLS. Required fonts and hashes belong to session configuration. |
| `js_lingo::builtins::SEED` (`js_lingo/builtins.rs:135`) | Thread-local RNG seed | Session/player RNG | Runtime TLS. Seed installation is explicit and reset-aware. |
| `flash_object::FLASH_OBJECT_COUNTER` (`handlers/datum_handlers/flash_object.rs:84-86`) | Thread-local object ID counter | Session/player Flash handle allocator | Runtime TLS. IDs must not collide across owners. |
| `transform3d::DIRTY_TRANSFORM_IDS` (`handlers/datum_handlers/transform3d.rs:8-12`) | Thread-local dirty set | Player render state | Runtime TLS. Dirty IDs are meaningful only in their owning player. |
| `havok_physics::IN_STEP_CALLBACK` (`handlers/datum_handlers/cast_member/havok_physics.rs:6-13`) | Thread-local reentrancy flag | Player physics service state | Runtime TLS. Native Xtra work remains deferred but must preserve browser isolation. |
| `gif::PENDING` (`gif.rs:192-193`) | Process-global mutex-protected pending animations | Session-owned loading/completion queue | Mutable runtime singleton. Async completion must carry owner and generation. |
| `budapi::{MOUSE_DISABLED,KEYS_DISABLED,SCREENSAVER_DISABLED,SOUND_VOLUME}` (`xtra/budapi/mod.rs:24-27`) | Process-global atomics | Session/player Xtra state | Mutable runtime singleton. These are movie-visible values and cannot be shared across sessions. |
| `xtra::{FILEIO_XTRA_MANAGER_OPT,MULTIUSER_XTRA_MANAGER_OPT,XMLPARSER_XTRA_MANAGER_OPT,CURL_XTRA_MANAGER_OPT}` | Process-global optional managers | Session-owned extension manager set | Mutable runtime singletons. File storage, network tasks, XML IDs, and sockets must be owner-qualified. |
| `xtra::scene3d::XTRA_SCENE_STORE` (`xtra/scene3d.rs:128-139`) | Process-global scene store | Session/player scene service | Mutable runtime singleton. Scene and mesh IDs must be scoped to the owner. |
| `xtra::sysmenu::STATE` (`xtra/sysmenu/mod.rs:51`) | Process-global optional state | Session/player sysmenu state | Mutable runtime singleton. |
| `xtra::external::{REGISTRY,PENDING_LOADS}` (`xtra/external.rs:95-189`) | Process-global external-Xtra names and waiters | Session-owned registry and owner-bearing pending-load table | Mutable runtime singleton. Completion must include name, session/player, and generation. |
| `physx_engine::NEXT_ID` and local Xtra ID atomics | Process-global counters | Session/player allocator | Mutable runtime IDs. IDs may be process-unique only for diagnostics; movie-visible handles must be owner-qualified. |
| `xtra_sdk::__XTRA_SDK_REGISTRY` (`xtra-sdk/src/lib.rs:87`) | Plugin-side `static mut` instance registry | Plugin instance state keyed by an explicit host owner | Mutable extension state. Preserve browser behavior; the SDK ABI may change when owner identity is required, and foreign instance IDs must be rejected. |
| `PLAYER_OPT` direct uses in `lib.rs`, `score.rs`, `flow_control.rs`, `sound_channel.rs`, and handlers | Ambient access from exported APIs and handlers | Context-bearing public API and handler signatures | Call-site migration surface. The inventory scanner must keep this list current until all accessors are gone. |

## Rust globals eligible for a narrow allowlist

These are candidates for process-global state only after the ownership scanner
records them as immutable data or diagnostics. A candidate does not authorize
runtime mutation without review.

| Symbol family | Reason it can remain global | Constraint |
| --- | --- | --- |
| `bitmap::palette` arrays, `bitmap::BUILTIN_TILES`, `QUICKDRAW_PATTERNS` | Immutable lookup data | Must be `static` immutable values. |
| `director::lingo::constants` lookup maps | `OnceLock` immutable opcode/builtin lookup tables | No movie/session display or dynamic symbol state may enter them. |
| `keyboard_map` lookup maps | Immutable key conversion tables | Configuration overrides belong to the session. |
| `interp_stats::{ENABLED,INTERP_OPS,ESCAPED_OPS}` | Process diagnostics | Counts must never drive VM behavior or owner lookup. |
| Logging-only `AtomicBool`/`AtomicU32` guards and logged-key sets in rendering/W3D | Diagnostics and duplicate-log suppression | Keep out of semantic state; document each exception. |
| Profiling aggregate/recording state | Process diagnostics | Tag records with owner identity if multiple sessions are profiled; no VM behavior may depend on it. |
| Test locks, test log IDs, and dotenv cache in `player/testing*` | Harness-only state | Exclude production ownership checks or explicitly mark test scope. |

`player_semaphone` and any mutex used to serialize the old singleton are not a
runtime ownership solution. They may remain only as temporary test/harness
coordination and must not guard a shared session in the native design.

## JavaScript and browser integration state

| Current symbol | Current storage | Target owner | Classification and migration requirement |
| --- | --- | --- | --- |
| `dirplayer-js-api::vmCallbacks` (`index.js:1-4`) | Module-global callback object | `DirPlayerSession` handle | Runtime singleton. Each callback closure must capture the session/player handle. |
| `_movieLoadedResolvers` (`index.js:10-28`) | Module-global resolver queue | Session-owned movie lifecycle queue | Runtime singleton. Resolvers must be rejected or resolved on reset/shutdown. |
| `_plugins`, `_vmModule`, `_xtraMovieBase`, `_xtraHostBase`, `_xtraRegistry` (`index.js:180-184`) | Module-global plugin/module/config state | Session-owned host services, with immutable plugin code shared only where safe | Runtime singleton/config ambiguity. A plugin's code may be cached globally, but instances, registries, URL bases, and host dispatch must be session-qualified. |
| `_pendingLoads`, `_onDemandInFlight` (`index.js:510,659`) | Module-global async state | Session-owned async resource service | Runtime singleton. Completion includes owner and generation; reset cancels or invalidates loads. |
| `flashPlayerManager::instances` (`flashPlayerManager.ts:65`) | Module-global `Map` keyed by sprite number | Session/player Flash service keyed by `(SessionId, PlayerId, SpriteNum)` | Runtime singleton and current key is insufficient for nested/concurrent sessions. |
| `flashLoadingCount`, `flashAccessBeforeReady`, `pendingNetCount` (`flashPlayerManager.ts:68-73,258`) | Module-global counters/flags | Session/player lifecycle state | Runtime singleton. Must not cause one session to stall another. |
| `pendingOps`, `pinTarget` (`flashPlayerManager.ts:1282-1298`) | Module-global maps keyed by sprite | Owner-qualified Flash operation queues | Runtime singleton. Queued operations are canceled on owner reset. |
| `callbackRegistry` (`flashPlayerManager.ts:1957`) | Module-global Flash-to-Lingo callback map | Per-session/player callback registry | Runtime singleton. Dispatch keys include session/player/sprite/cast identity. |
| `window.fetch` and canvas/context monkey patches (`flashPlayerManager.ts:249-362`) | Page-global host hooks | Host transport service with explicit session configuration | Browser integration state. Keep one page hook if necessary, but route configuration and pending counts by session. |
| `ruffleBridgeClient::{nextRequest,pendingRequests,eventHandlers}` (`ruffleBridgeClient.ts:22-29`) | Module-global transport maps | `RuffleBridge` instance, with player/session IDs in every request | Transport singleton. Page-level event listeners may remain, but no callback can resolve without its owner. |
| `syncNode`, `nextSyncId` (`ruffleBridgeClient.ts:142-155`) | Module-global DOM channel and counter | Bridge instance or owner-qualified sync transaction | Transport state. Concurrent sessions must not overwrite one another's synchronous result. |
| `public/dirplayer-ruffle-bridge-host.js::players,nextId` (`:23-24`) | Page-level main-world player registry | Explicit bridge host registry keyed by session/player handle | Existing IDs are explicit, but creation, callbacks, and destruction must validate owner and generation. |
| `window.dirplayer_RufflePlayer`, `dirplayer_localConnectionSend`, and bridge globals | Page-global names/hooks | Namespaced host transport endpoint | Browser integration surface. Retain only as a boundary; never use it as implicit VM state. |
| `polyfill::mountedRoots` (`polyfill/src/core.tsx:407`) | Page-global DOM mount map | DOM host lifecycle | UI state, not VM state. Keep page-scoped but ensure unmount disposes its session. |
| `extension/src/page-fetch.ts::nextId` | Extension page transport counter | Extension transport request namespace | Transport-only state; response routing must not be confused with VM ownership. |
| Redux store and `localStorage` in `src/vm/callbacks.ts` and views | Application/UI global state | UI instance keyed by the owning player/session | UI state. Do not let UI selection or timeout handles become VM ownership. |

## Explicit context and handle contract

The migration should introduce an explicit `ExecutionContext<'a>` (or an
equivalent name) containing a borrowed runtime session and a `PlayerId`. It is
passed through interpreter dispatch, handlers, rendering, callbacks, and Xtra
host operations. A context may borrow the owning player for synchronous work;
it must not be retained across an external wait.

Externally retained values use an immutable owner token containing:

- `SessionId`, `PlayerId`, and generation/epoch;
- an `Rc`-backed lifecycle record with `Cell` metadata for validity/reset state;
- no pointer to `DirPlayer`, `RuntimeSession`, or another mutable VM object.

The token is metadata only. The allocator and bitmap manager remain borrowed
from an explicit context. This preserves thread affinity and prevents a VM
reference from keeping an entire mutable runtime alive.

Foreign or stale references are checked against the current owner token and
epoch before raw arena access. A failed check returns `Void`/`None` or a typed
script error according to the API; it never reads the ID or refcount slot.

## Reclamation and reset boundary

Reference drops cannot borrow a player or bitmap manager directly. While the
owner epoch is still arena-live, they decrement the entry's raw count and
append an owner-qualified reclamation item only when that count reaches zero.
The queue contains only IDs, epochs, and the minimal effect description needed
to finish reclamation; it contains no VM pointer. Thus the drop decrements the
raw count once, and the later drain only removes an entry whose count is already
zero; the drain never decrements the same count again.

The owning scheduler drains the queue at explicit safe points:

1. after a synchronous command/event/handler boundary returns and all temporary
   VM borrows have ended;
2. after an async completion is accepted, before the next script event;
3. immediately before capture/render submission, so bitmap effects are visible
   in the same deterministic boundary;
4. during reset, after clearing player roots but before invalidating the old
   arena epoch; and
5. during shutdown, after canceling owned work and before releasing the owner
   token.

Draining borrows the owning allocator and bitmap manager explicitly. For each
item it validates the epoch, checks that the arena slot still exists and has a
zero count, then removes it; it never decrements the count a second time.
Removing the entry drops its child `DatumRef`s, which recursively enqueue their
own zero-count work. If the entry contains an ephemeral bitmap, the drain
applies `decref_ephemeral` through that same owner after releasing the allocator
borrow.
Anchored bitmap references remain unaffected. Items from an invalidated epoch
are discarded without arena access, so reset cannot use freed refcount cells.

Reset sequence:

1. cancel or mark all owned async work stale;
2. enter a reset state that rejects new allocations and semantic arena access.
   The old arena remains addressable only for explicit processing of items
   already queued before reset; external and arena-entry Drops during
   `Resetting` skip raw refcount access and do not enqueue new work;
3. clear player roots and repeatedly drain the old epoch queue to completion.
   Each already-queued zero-count datum is removed and any returned ephemeral
   bitmap effect is applied through the old owner's borrowed bitmap manager.
   Child references dropped as part of this reset teardown are covered by the
   remaining-entry bitmap sweep below rather than by recursive Drop queueing;
   this loop ends only when no valid reclamation item remains;
4. mark the old epoch arena-dead, then clear remaining arena entries. Drops
   caused by this teardown skip raw count access because the arena is already
   invalidated. Before this point, explicitly release any remaining ephemeral
   bitmap counts that are still represented by arena entries, so clearing the
   arena cannot strand bitmap-manager counts;
5. discard the old queue, advance the owner generation, and construct fresh
   runtime state with a fresh owner token; and
6. discard any late completion or stale reference before it reaches the new
   runtime.

The reset path must not set a global flag that changes the meaning of drops for
another session. This order preserves recursive datum and ephemeral bitmap
reclamation while making late `Drop` calls harmless. The explicit arena-live
transition and final bitmap sweep are required because entries that still have
nonzero counts are destroyed as part of whole-arena reset rather than through a
zero-count queue item.

## Migration sequence and gates

1. **Inventory gate:** land this document and the scanner/audit tests. Record
   the clean `native-bevy` baseline and browser regression result.
2. **Reference gate:** add owner tokens, epochs, validation, and reclamation
   queue tests. `DatumRef::Drop` and `ScriptInstanceRef::Drop` must have no
   active-player lookup and no pointer to the mutable VM.
3. **Session shell gate:** create `RuntimeSession`, typed player slots, per-
   session symbols, scheduler endpoints, and Xtra manager ownership. Migrate
   command/event loops and exported entrypoints through explicit contexts.
4. **Interpreter gate:** migrate handlers, bytecode state, JS-Lingo state,
   allocator access, and callbacks; remove the old reserve/accessor functions.
5. **Rendering/audio/Xtra gate:** move renderer, bitmap/font caches, audio,
   Flash lifecycle, network completion, and extension state behind session
   services.
6. **Browser/native gate:** convert the JavaScript API and Bevy frontend to
   disposable instance handles, then prove interleaved same-thread and
   separate-thread sessions, reset during pending work, and teardown isolation.

## Known allocator blockers for the next increment

The current allocator increment closes stale `DatumRef` and
`ScriptInstanceRef` reclamation through owner tokens, but two existing bitmap
ownership paths remain outside that guarantee and are deliberately deferred:

- mutable datum APIs can change a live arena entry from or to `BitmapRef`
  without applying the corresponding bitmap-manager increment/decrement; the
  next allocator increment must account for variant replacement effects while
  retaining the entry's validated refcount identity;
- raw `BitmapRef` values can still be constructed or transferred through
  existing cast-member and cross-player paths whose ownership is represented
  by the bitmap manager rather than the datum owner token. The current
  manager-aware allocation path accounts for ephemeral bitmap effects, but
  full bitmap isolation and foreign bitmap rejection require the session
  migration.

These are explicit blockers for full session ownership and are not claimed as
resolved by this increment.

Acceptance requires a static audit showing no mutable runtime globals or
implicit current-player lookup remain. Process-global entries must match the
allowlist above and be demonstrably immutable or diagnostic-only.

## Synchronous opcode transitive dependencies

Source review status: these are unimplemented migration dependencies. They
have no runtime proof and extend the call-site migration surface; converting a
direct opcode wrapper to accept `ExecutionContext` does not resolve these
transitive boundaries.

| Opcode/caller chain | Residual dependency | Required migration boundary |
| --- | --- | --- |
| `bytecode::get_set::GetSetBytecodeHandler::set` -> `score::sprite_set_prop`; also `script::player_set_obj_prop`, `handlers::datum_handlers::sprite`, `js_lingo::interpreter`, and score-internal callers | `sprite_set_prop`, `borrow_sprite_mut`, and `sprite_set_prop_is_noop` use ambient player access, including a raw `PLAYER_OPT` mutation. The setter directly invokes Ruffle frame control and `JsApi` channel callbacks. | Pass `ExecutionContext`/explicit player through all three helpers and callers. Queue owner/generation-qualified Ruffle and `JsApi` effects for draining after the player borrow. |
| `bytecode::get_set::GetSetBytecodeHandler::set` -> `handlers::datum_handlers::cast_member_ref::CastMemberRefHandlers::set_prop`; also `script::player_set_obj_prop` and chunk-property fallback paths | `set_prop` has no player parameter, reserves the ambient player through member-type setters, and dispatches cast-member `JsApi` notifications. | Add explicit context to `set_prop`, member-type setters, and member mutation helpers. Queue cast-member notifications at the owner boundary. |
| `bytecode::get_set::GetSetBytecodeHandler::set_obj_prop` -> `script::player_set_obj_prop` | The async dispatcher extracts values and executes most datum branches through ambient reserves; the CastLib branch awaits an async helper. | Use the owned context for synchronous mutation, then split or return an owned continuation/effect for async CastLib and host work. Do not await while borrowing the player. |
| `script::get_obj_prop` (String branch) -> `StringDatumUtils::get_built_in_prop` -> `eval::try_eval_lingo_expr_static` -> `eval::eval_lingo_pair_static` | String `.value` recursively evaluates through ambient allocator/player and object-property access; `.marker` and `.numberOfItems` also reserve the player. | Give the string utility and static evaluator explicit context, including symbol/allocator ownership; preserve any future host mutation as a queued effect. |
| `bytecode::get_set::GetSetBytecodeHandler::get`/`get_chained_prop` -> `score::sprite_get_prop` | The getter accepts a player but re-enters ambient state for behavior and script-list/custom-property paths. It directly reads Ruffle play/frame/hit-test bridges. | Remove nested reserves and use the passed context. Route bridge reads through an explicit host service or owner-scoped effect boundary. |
| `handlers::datum_handlers::mod::player_call_datum_handler` -> `StringDatumHandlers`, `StringChunkHandlers`, `PropListDatumHandlers` | Dynamic property methods retain ambient reserves; `PropListDatumHandlers::get_prop_ref` directly calls `player_mut`. | Migrate handler entrypoints and nested helpers to `ExecutionContext`, with owner-qualified deferred work where callbacks or host services are involved. |
| `bytecode::get_set` sound/stage property branches -> `SoundChannelDatumHandlers::{get_prop,set_prop}` and `stage::{get_stage_prop,set_stage_prop}` | These helpers accept a player but perform direct WebAudio, shared-renderer, and `JsApi` work while it is borrowed. | Keep VM state mutation in context, and move browser audio/render/callback operations behind explicit session services or queued effects. |

The synchronous bytecode string operations themselves have no remaining
`reserve_player_*` call in their bodies; their warning-only `web_sys` logging is
separate from the ambient-player dependencies above. Explicit helpers such as
`script_get_prop`, `PropListUtils`, `ListDatumUtils`, `StringChunkUtils`, and
context-variable access remain safe only when their callers preserve the
explicit player/context contract.
