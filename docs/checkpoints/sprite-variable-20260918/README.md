# Accepted sprite-variable ownership component

Navigator accepted this bounded Stage 2.6 component locally on 2026-09-18.
It is unpublished; full Stage 2 remains open.

Sprite getVariable/setVariable now preserve attached-script precedence and run
host effects through captured owner, sprite, cast and generation capabilities
outside mutable VM borrows. Early absent/unresolved object handles can associate
with their first exact publication. Replacement, reload, invalid resolution,
failed publication, reset and disposal retire that authority. Old-instance
cleanup carries exact retired authority and cannot unload a replacement.

The sprite-specific getter preserves scalar conversion and strict stored-object
paths without changing global getter or FlashObject routes. Actual embedded
Ruffle tests retain early cast-0:0 objects, read and invoke through them after
publication, preserve stored-object identity after source-variable reassignment,
reject old objects after replacement and keep the neighboring owner usable.

Verification: native 628/0, frontend manager 15/15, lifecycle 38/38, focused
browser 1/0 and combined browser 21/0. WASM check, locked/offline production
package and production frontend build pass. Navigator verified the final source,
artifact and evidence hashes, including bundled WASM matching the package.
Actual MV3 runtime qualification and Canvas2D linked-Movie composition are not
claimed by this component.

`acceptance.json` records all identities and limits. The source overlay at
`~/Projects/.dirplayer-work/sprite-variable-stage2/accepted-source-overlay.tar.gz`
contains 62 files over the recorded repository/submodule heads, preserving the
previous accepted components. Raw receipts and manifests remain in that same
work directory. Historical failures and review corrections are retained in
`../sprite-variable-ownership-audit-20260918.md` and the validation summary.

Next bounded implementation: the reviewed timeout contract in
`../timeout-host-ownership-audit-20260917.md`. Stop before Stage 3.
