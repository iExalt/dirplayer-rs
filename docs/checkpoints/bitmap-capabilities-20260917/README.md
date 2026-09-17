# Bitmap capability checkpoint — 2026-09-17

Accepted component of Stage 2.1 over `3d08241`; the complete Stage 2 exit gates
remain open. The source manifest identifies this snapshot independently of
subsequent worktree changes.

Retained bitmap values now carry a private manager/generation capability.
Reads, writes, replacement and refcount operations reject foreign or stale
handles before accessing bitmap storage. Allocator publication increments the
bitmap count transactionally; nested reclamation and reset sweep the old handles
before retiring their generation. Reset preserves anchored cast pixels and
clears transient images. Explicit cross-manager copies resolve source palettes
and the supplied default into independent RGBA pixels.

Deferred access and duplication also check nested datum/script/bitmap/symbol
references. The graph validator checks provenance before suppressing visited
numeric IDs, including cyclic lists and colliding IDs from separate owners.

## Verification

- Corrected native source: **501 passed, 0 failed**, none ignored or filtered.
  Cargo selected the exact library executable after successful fresh test compilation.
- Corrected WASM test compilation: **exit 0**.
- Browser ownership fixtures: **13 passed, 0 failed**. The first sandbox run
  failed to bind the local server before executing fixtures; the elevated retry passed.
- Frontend Flash tests: **34 passed, 0 failed**; targeted TypeScript: **exit 0**.

The browser/frontend/TypeScript receipts precede the final native test additions
and allocator error-code correction from Generic to InvalidReference. Their
browser behavior and interface remain unchanged; final native/WASM checks cover
the corrected source. Direct native handler tests exercise image creation,
duplicate/copyPixels and cast-image get/set. No licensed image movie fixture was
available; this is not a full movie campaign or native-player acceptance.

See [commands and results](verification-summary.json),
[relevant executed native tests](native-relevant-results.txt), and the
[source manifest](runtime-source-manifest.tsv). Raw logs remain at the local
evidence path recorded in the summary. No build/dependency caches are copied.

An accidental recursive formatter run was recovered before validation: 126
formatter-only paths were restored, and intended changes were reconstructed
and reviewed. Existing Ruffle edits and `mise.toml` were preserved.
