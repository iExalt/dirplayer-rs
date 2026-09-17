# Typed mutation checkpoint — 2026-09-17

Accepted Stage 2.1 component following the bitmap capability checkpoint.
The complete reference/mutation audit and Stage 2 exit gates remain open.

`DatumAllocator::replace_vector` and `replace_transform3d` validate owner,
liveness, arena entry and refcount pointer, reject immortal entries, and require
the exact existing variant before replacing a scalar payload. Three vector,
six transform and one persistent Shockwave transform write use these methods.
The persistent transform path returns before changing its cache when replacement
fails. `DirPlayer::get_datum_mut` is now crate-private.

The native tests exercise successful replacement, wrong-type rejection,
foreign and stale handles with colliding live IDs, pooled symbol reuse after
rejection, and unchanged bitmap lifetime/refcount. Existing parent-transform
writeback coverage also passes.

## Verification

- Exact Cargo-selected library binary: **503 passed, 0 failed**, none ignored
  or filtered. Both reviewed `typed_replacement_*` tests ran.
- Locked/offline WASM test compilation: **exit 0**.
- Complete source manifest unchanged before/after validation.
- No browser rerun: no JS/host interface changed in this component.

An initial integration differed from the reviewed staging. It was corrected
to the exact reviewed methods, tests and callsite changes while preserving the
newer bitmap error-code fix. The earlier 503-test result ran the superseded
integration and is **not accepted evidence**. The definitive run additionally
checked exact test names before execution; all accepted artifacts use the
`corrected-final` prefix.

See [commands and results](verification-summary.json),
[executed relevant tests](native-relevant-results.txt), and
[source manifest](runtime-source-manifest.tsv). Existing Ruffle edits and
`mise.toml` were preserved; no formatter or dependency-cache copying occurred.
