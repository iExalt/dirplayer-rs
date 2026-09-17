# Iterative same-owner duplication — 2026-09-17

Accepted bounded Stage 2.1 duplication component, integrated. This does not close Stage 2.1 or
the combined Stage 2 cutover; the combined Flash browser gate remains pending.

The production `player_duplicate_datum` wrapper delegates to `datum_duplicate`;
the recursive inner implementation is removed. Ownership and bitmap validation
plus cycle preflight run before destination allocation. Iterative reconstruction
preserves per-occurrence duplicate semantics, List/PropList ordering and sorted
flags, nested VOID values, and independent bitmap storage. Ordinary leaf variants
retain their existing clone behavior. OOM/panic rollback is not claimed.

Six focused tests passed from the fresh Cargo test artifact, then all 554 native
tests passed. The frozen-source WASM tests check passed, and all 378 Rust source
hashes matched the recorded manifest. The focused output, artifact identity, source manifest and command
receipt are retained here. The manifest also covers concurrent Flash changes;
those have separate browser acceptance requirements.
