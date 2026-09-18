# Sprite variable ownership follow-up

Accepted locally on 2026-09-18: see `sprite-variable-20260918/README.md` and
`acceptance.json` for final evidence and limits. Native 628/0, frontend 15+38,
focused browser 1/0, combined browser 21/0 and production package/frontend gates
pass. Full Stage 2 remains open. The historical discovery and correction notes
below preserve earlier incomplete and failed states; they are not current status.

SpriteAsync setVariable still uses the shared numeric readiness block, then
calls SpriteDatumHandlers::call inside the mutable session borrow. Its host
setter uses a local sprite number without exact owner and generation. Discovery
corrects the initial getter inventory: classify_async_object already shortcuts
SpriteRef.getVariable into BindGet, requiring a path and bypassing SpriteAsync
attached-script precedence. Its missing-member cast pair 0:0 cannot pass the
executor's real-member check. The legacy synchronous getter body below remains
the compatibility specification, not a satisfactory owned execution route.
The dedicated callFunction repair does not close these gaps. Global getVariable
and FlashObject operations also require separate regression coverage.

A bounded follow-up should prepare checked inputs and captured binding under
the selected player, perform host work outside the borrow, and revalidate before
allocating results. Proposal review must preserve the source's observable input
and result rules rather than reuse a generic converter without comparison:

- Getter defaults to empty path/value mode when arguments are absent. Its second
  consumed argument selects object mode only when integer conversion yields zero.
- Object mode currently preserves primitive strings, treats object-coercion
  strings as references, and returns a sprite-bound reference for other results.
  It also promises a handle before the sprite member resolves. Define the exact
  owner/generation capability for that early case before changing it; do not
  silently adopt a later replacement or erase this behavior without a decision.
- Value mode guards the current loaded cast pair and returns string, bool as
  integer, integral number as integer under the existing strict i32 bound, other
  number as float, and other values as Void.
- Setter consumes path and value as strings and returns Void. Preserve ignored
  trailing arguments and define checked errors for missing consumed arguments.

Require two handles with overlapping sprite IDs, exact-generation replacement,
host reentry, no borrowed host work, consumed foreign/stale reference rejection,
early access and explicit native unsupported behavior. Test actual sprite-method
entrypoints, including real object-mode use; the callback fixture obtains its
object from a callback and cannot certify object getter transport.

The browser runner's bridge converts object-valued getter results to null. Any
required transport repair must be proposed explicitly and distinguished from
full MV3 extension qualification. Keep other sprite playback/input methods and
timer ownership outside this component.

## Proposal review

The proposed common SpriteAsync route restores override precedence and prepares
checked get/set operations before detached host work. A provisional owner-local
generation would let an early object handle bind the first valid publication
without adopting a replacement. Before implementation, specify invalidation for
intervening non-Flash assignment, clear/replacement, reload and failed publication;
distinguish absent, unresolved and non-Flash members. Validate generation overflow
and repeated early-get reuse.

The shared classifier currently checks every argument before choosing the
request kind. Ignored-extra semantics therefore require full entrypoint review,
not only tests of a preparation helper. Attached scripts may consume additional
arguments and must retain checked access.

A separate owned bridge getter may preserve primitive types and strict retained
object markers from the existing Ruffle producer. Define its exact shape/path
validation and affected owned/global-builtin callers; preserve the legacy numeric
bridge API. Full MV3 qualification remains outside this component.

The classifier contract is now explicit: all retained argument references must
be live and owned before deferral; only conversion/use of valid trailing builtin
arguments is ignored. Attached scripts receive the complete validated vector.

Frontend implementation is approved for a separate sprite-specific owned getter
endpoint, preserving primitive JSON types and accepting only the producer's
single own __dirplayer_stored_path field with a canonical rooted u32 marker path.
Legacy numeric bridge conversion, global BindGet and FlashObject routes remain
unchanged. No Ruffle rebuild is required.

The first Rust lifecycle proposal required a corrected state transition design. A generic
"invalidate on every assignment" rule conflicts with consuming the first absent
reservation; the allowed first association must be explicit and atomic. Preserve
early handles for exact present-but-unresolved cast references as well as absent
members, invalidating on unrelated assignment, clear, reload or failed publication.
Do not conflate unresolved data with a resolved non-Flash/invalid-SWF target.

## Approved implementation boundary

The revised lifecycle is approved. Prepublication origins distinguish Absent,
Exact(pair) and FirstPublished(pair), always carrying a checked generation.
An atomic Stage member transition allows Absent/old=None to associate its first
exact pair without changing generation. The same unresolved pair may resolve
through initial preload; clear, unrelated assignment, resolved non-Flash/invalid
data, reload, failed publication and reset retire the capability. Repeated early
gets reuse an active reservation; exhaustion never wraps or mutates state.

Core pilot owns player/mod.rs, score.rs, driver.rs, sprite.rs, flash_object.rs and
commands.rs if required, plus native tests. Frontend pilot owns the separate
bridge helper/manager and tests. The Sol lead owns integration/builds and will
assign browser fixture ownership after reviewing both production packets.

Retired instance cleanup needs explicit authority: normal current-generation
fences must not suppress the old instance's necessary unload, and that unload
must never affect a newer/reentrant replacement or make the retired capability
request-current again. Test cleanup/reentry and first-publication failure as
well as owner collisions, consumed-reference checks and early access. Audit all
production Stage member writes and retain justified FilmLoop-local exceptions.

Store new component evidence separately from the accepted callback snapshot.
No Ruffle edits/rebuild, commit or push is authorized for this increment.

Frontend first-pass packet: the lead reviewed the separate bridgeGetVariableOwnedSync
helper, strict marker normalization, exact-generation sprite getter route and
focused tests. The pilot reports manager tests 15/15; legacy bridge conversion
and global/FlashObject endpoints remain unchanged. This is preliminary frontend
evidence only. Rust lifecycle implementation and integrated browser proof remain
pending; final frozen-source checks must follow integration.

The lead independently reran frontend manager tests: 15/15 at
`~/Projects/.dirplayer-work/sprite-variable-stage2/validation/frontend-manager-focused.log`.
Rust first review returned lifecycle corrections (premature reservation promotion
and multiple retired-generation storage), plus missing early object handles and
the still-unwired sprite-specific getter operation. Those are bounded repairs
within the approved contract; no native compilation or integrated runtime pass
is claimed at this checkpoint.

The first frozen offline Rust check now reaches source and reports seven bounded
compiler errors (sprite numeric/reference types, an Option wrapper and consumed
action finalization). The lead assigned one coherent repair batch without cloning
SWF payloads. Receipt:
`~/Projects/.dirplayer-work/sprite-variable-stage2/validation/rust-check-compiler1.log`.
No successful Rust or integrated runtime gate is claimed yet.

Latest lead review: the seven compiler errors are repaired. Before the next
freeze/check, the lead requires owned unload to prove exact membership in the
retired-generation set, and fixes a fallback that could queue a current
generation without retiring it. Three focused state/production-path tests are
part of that bounded correction. Frontend remains reviewed and green; no scope
change or new runtime acceptance is implied.

Navigator acceptance review of the preliminary production packet found remaining
lifecycle gaps: cast-library reload must retire unresolved reservations even
without a loaded tuple; an older retired same-member generation must not hide
the current generation during reload; and pre-dispatch must reconcile unresolved
reservations that resolve to invalid or non-Flash content. The lead owns one
bounded correction with production-hook tests and an audit of draining all
retired teardown authorities. Earlier focused green checks do not establish
acceptance of these paths.

The sprite object-mode decoder also needs to consume the strict stored-path
marker preserved by the new host route. Falling back to the requested variable
path for a valid marker loses retained object identity when that variable is
reassigned. Browser acceptance must retain the original object across that
reassignment. Browser fixture work continues independently while the lead
coordinates these corrections; no integrated browser result is recorded yet.

Corrected Rust compilation passes, but the first production cast-reload test
failed: retiring an unresolved generation left its origin marked Exact. The
lead retained `sprite-variable-stage2/validation/native-cast-reload-final.log`
under `~/Projects/.dirplayer-work` and paused subsequent focused checks while the
core pilot makes successful current retirement clear its origin. Stage member
transition then establishes only the intended next origin. This failure is
historical failing evidence, superseded by the corrected focused run below.

After the retirement-state repair, the lead reports corrected exact-source Rust
check and focused native results: binding state 12/0, cast reload 2/0,
pre-dispatch 2/0, failed publication 2/0, marker validation 1/0, sprite get 3/0
and sprite set 2/0. Corrected receipts use `*-final2.log` in the same validation
directory. Navigator interface review confirms pending cast bindings participate
in reload, retirement clears the origin, reconciliation classifies unresolved
versus invalid content, and object decoding consumes canonical stored paths.
The host route normalizes marker shape before VM decoding. Full native,
WASM/browser and production package gates remain pending; the component is not
accepted.

The first integrated packet passes native 628/0, frontend manager 15/15,
lifecycle 38/38, the focused browser fixture and combined browser 21/0.
Production packaging encountered a sandbox wasm-opt restriction and is being
rerun with reliable exit capture outside the sandbox. Final acceptance review
also found that the fixture checked the early cast-0:0 handle's generation but
did not use that retained handle after publication. The lead owns a fixture-only
correction: retain it in Lingo, read through it after first publication, exercise
its method route, and reject it after same-owner generation replacement. Earlier
browser/package receipts cannot establish this added required assertion; rerun
them against the corrected fixture source before accepting the component.

The corrected fixture now retains the original cast-0:0 objects in Lingo globals,
reads their published property, invokes the authored function through the early
object, checks its argument effects, and rejects the early object after same-owner
replacement. Navigator inspected these assertions. The corrected production
WASM package and frontend build are green at
`wasm-pack-production-early-handle-final.log` and
`frontend-production-build-final2.log`; browser validation is still pending.
