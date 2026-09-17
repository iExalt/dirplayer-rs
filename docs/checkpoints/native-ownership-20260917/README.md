# Native ownership WIP publication checkpoint

This package preserves work without integrating or accepting new runtime behavior.
The implementation remains paused. The source commit is based on audio fix
`297da4a410495a1116e2c1b93ee57f9f1b5c8d79`, whose parent is upstream main
`68376fbb4494a6bbad4c70081ecdcb99814a74c9` at publication.

## Publication checklist

- [x] Preserve the live ownership implementation, documentation, tooling and tests.
- [x] Preserve the combined review delta and validate its applicability to live source.
- [x] Preserve uncommitted Ruffle changes and the separate uncompiled callback proposal.
- [x] Preserve the unintegrated JS-Lingo proposal as historical source material.
- [x] Retain compact verification records without build caches or generated binaries.
- [ ] Integrate the combined review delta and rerun acceptance on the resulting source.
- [ ] Review/rebase the Ruffle callback and JS-Lingo proposals before integration.

## What is and is not verified

`verification/` records native/WASM compilation, 480 native library tests,
13 actual browser runtime fixtures, and 34 frontend tests for the **review stage**.
The recorded targeted TypeScript configuration accompanies that checkpoint;
its successful result is recorded in the implementation log, not a new run here.
These results do not certify the live source or the complete native-player plan.
Build artifacts, licensed movies, dependencies and caches are intentionally absent.
Commands and logs retain original absolute paths for provenance; adapt paths
and provision dependencies/fixtures before reproducing them.

`combined-review.patch` reconstructs 37 changed/new files over this publication's
live source. Its before/after hashes are in `combined-review-manifest.json`.
It includes the exact staged mise.toml (which lacks the live incremental-disable
setting); preserve `CARGO_INCREMENTAL=0` explicitly when reproducing that stage.
The patch is retained, **not applied**. Original source whitespace is preserved.

`ruffle-working-tree.patch` is relative to the pinned Ruffle submodule commit.
It preserves seven local edits; the gitlink remains unchanged and the local
submodule stays dirty. No third-party Ruffle branch was changed. Apply this
patch inside the submodule on a fresh checkout to reproduce those edits.
`callback-ruffle-callback-source.patch` is a separate uncompiled proposal with
before/after manifests; inspect its paths and baseline before applying it.

The JS-Lingo patches are stale, unintegrated proposals. They overlap subsequent
refactors and must not be applied wholesale. Their presence is preservation,
not evidence that JS-Lingo ownership is complete.

`published-source-manifest.json` describes live source before this evidence
package was added. `live-baseline.json` describes the last accepted integration
boundary. See the implementation checklist for remaining milestone gates.
