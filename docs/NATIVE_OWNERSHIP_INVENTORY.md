# Native DirPlayer ownership inventory

Status: Stage 1.2 current-tree classification; no Stage 2 cutover claim.

The authoritative per-finding index is
[`native-ownership-classification.json`](native-ownership-classification.json).
Every lexical finding has a category, source path, symbol, occurrence number,
matched-text hash, disposition, target owner, roadmap requirement owner and
rationale. Line numbers are navigation hints. Repeated symbols are separate
occurrences; a new, removed or changed occurrence invalidates coverage.

The manifest records the parent and Ruffle revisions plus hashes of every
scanned source file, including files without findings. These hashes include the
pre-existing dirty Ruffle integration. It also pins the scanner and records its
coverage roots, exclusions and limitations. Changed source, new source files,
missing source roots and changed scanner behavior require renewed review.

Run `mise exec -- python scripts/check-runtime-ownership-classification.py`
to check inventory coverage. Run
`mise exec -- python scripts/test_runtime_ownership_classification.py` for the
coverage checker's regression tests. This is deliberately separate from
`mise run check:ownership`: that removal gate still fails while unresolved
runtime globals, TLS, accessors and task wrappers remain.

Current receipt: **1,311 findings**: 566 access occurrences, 74 task
occurrences, 297 unresolved state candidates, 337 lexical review candidates,
29 immutable lookup entries, five harness entries and three diagnostic entries.
The scanned source set contains 900 Rust and 142 JavaScript/TypeScript files;
209 findings are under Ruffle. Scanner test and coverage receipts are retained
in [`checkpoints/stage1-inventory`](checkpoints/stage1-inventory/).

## Classification and acceptance meaning

- `unresolved-state` assigns a component owner to a state or host-binding candidate.
- `unresolved-access` records an access, import or definition occurrence; it is
  not a new singleton and does not establish that the call is defective.
- `unresolved-task` assigns review of capture, lifetime, cancellation and reentry.
- `lexical-candidate` assigns semantic investigation of scope and mutation.
  Local bindings, destructuring and unknown static types are deliberately not
  promoted to singleton defects or silently exempted.
- `immutable-data` is limited to reviewed fixed palette, tile, pattern and
  read-only opcode/key lookup tables. Session configuration cannot enter them.
- `diagnostic` is limited to the inspected interpreter instrumentation switch
  and counters. They must never control semantic VM behavior.
- `test-harness` identifies test lock/configuration/log/session-counter state;
  test serialization cannot establish production isolation.

An assigned unresolved owner satisfies the inventory gate, not the migration
requirement. Owners are roadmap component IDs, not claims that every candidate
must move or that every existing owned path is broken. Most logging-looking
atomics and Ruffle static types remain assigned review candidates rather than
receiving an unverified diagnostic or immutable exemption.

| Requirement owner | Target boundary |
| --- | --- |
| 2.1 | Player values, allocator tokens and session symbols |
| 2.2 | Explicit interpreter and handler execution context |
| 2.3 | Session scheduler and continuation lifecycle |
| 2.4 | Browser handles, host events, UI and extension page lifecycle |
| 2.5 | Browser Xtra/SDK instances and resource operations |
| 2.6 | Owner-qualified Flash operations |
| 2.7 | Ruffle callback origin and player/host bridge |
| 2.8 | JS-Lingo runtime, object registry, invocation and RNG |
| 2.9 | Residual player graph, rendering, audio, loading and service state |
| 2.10 | Integrated removal gate; all component owners feed this acceptance |

## Current source changes versus the historical inventory

The earlier symbol/line tables described a pre-migration tree. They are replaced
by the hash-pinned current index rather than retained as current facts.
`OwnerToken` in `player/ownership.rs` now carries identity, epoch state and a
reclamation queue, and `DatumRef::Drop` checks that token. `SymbolTable` is an
instance with owner identity rather than the old `SYMBOL_TABLE` singleton.
`RuntimeSession` and owner-qualified browser/Flash maps exist. These observations
are source facts, not a fresh lifecycle test or complete isolation proof.

Residual `PLAYER_OPT`, `ACTIVE_PLAYER_ID`, nested-player registries, channel
senders, generation state and `PLAYER_SESSION_HANDLE` TLS remain indexed.
JS-Lingo maps/RNG, renderer TLS, palette/quantizer/font preferences, GIF work,
BudAPI state and SDK instance registries remain assigned to their component
owners. Ruffle `LINGO_CALLBACKS` is a global callback vector whose entries lack
session/player/generation origin. Ruffle `CURRENT_CONTEXT` holds a raw context
pointer temporarily installed across an external callback. Both remain explicit
2.7 work even where outgoing Flash requests already carry tickets.

## Manual supplement and lexical blind spots

The scanner covers production Rust, JS/TS, extensions, Xtras, and Ruffle source
roots listed in manifest `coverage`; generated `public/ruffle` output, dependency
and test trees are excluded there. Macros, aliases, computed properties,
prototype writes and state assembled indirectly still require semantic review.
The following reviewed surfaces are included in whole-file source hashes:

| Surface | Finding / owner / required review |
| --- | --- |
| `src/services/flashPlayerManager.ts`: `origFetch`, `trackFetch`, `origGetContext`, `getSocketProxyConfig` | Owner 2.6: fetch pending count is page state read by Rust; the prototype `getContext` patch and `win` aliases escape direct-global-write matching. Keep page installation explicit, route configuration/counts by owner, and inspect teardown/reentry. |
| `dirplayer-js-api/index.js`: `routeFlashPlayOwned`, `routeFlashLocalConnectionSendOwned`, `win` installation | Owners 2.5/2.6: page functions route via `vmCallbacksByOwner`, while legacy `vmCallbacks` remains. Verify registration/disposal and legacy fallback before certifying concurrent owners. Alias installation is not independently proven by a lexical write match. |
| `public/dirplayer-ruffle-bridge-host.js`: IIFE `players`, `playerOwnerKeys`, IDs and handlers | Owner 2.7: closure-scoped page state persists beyond calls. Registry scope is host lifetime, but callback origin and disposal must preserve player/generation. |
| `ruffle/web/src/external_interface.rs`: `JavascriptInterface::call_method` | Owner 2.7: inspect temporary `CURRENT_CONTEXT` pointer replacement/restoration and nested callbacks; source scan does not prove lifetime safety. |
| `ruffle/web/packages/core/src/internal/player/inner.tsx`: `LOADED_METADATA`, `LOADED_DATA` | Owner 2.7: class static event-name strings are writable bindings; investigate writes before immutable exemption. Per-instance player fields also need lifecycle review although they are not global declarations. |
| `vm-rust/src/player/interp_stats.rs`: `counters!` expansion | Process diagnostics under owner 2.9: macro-generated AtomicU64 counters are not separately expanded by this scanner. Keep them instrumentation-only; whole-file hashing invalidates this review if definitions or uses change. |
| `vm-rust/src/player/ownership.rs`, `allocator.rs`, `datum_ref.rs`, `script_ref.rs`, `session.rs`, `symbols/symbol_table.rs` | Owner 2.1 with 2.3: instance fields are not singleton findings. Preserve explicit identity/epoch checks, reset ordering, reclamation and symbol lifetime; source inspection does not replace reference/lifecycle acceptance. |

No finite lexical scan proves the absence of hidden mutable state. Stage 1
acceptance is reproducible source coverage and explicit ownership of all emitted
findings plus this manual supplement. Semantic removal, multi-owner safety and
native/WASM/browser integration remain Stage 2 requirements.

The following architecture contract remains the target for later stages; it
is not a statement that its implementation is complete.

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
