# JS object ownership and value evaluation checkpoint — 2026-09-17

Accepted bounded component of Stage 2.8, integrated. The
complete Stage 2 and Stage 2.8 consumer audit remain open.

Runtime and object registries belong to the session. Retained JS objects carry
an opaque owner capability; colliding local IDs, stale generations and foreign
owners cannot redirect get/set/call operations. Object allocation checks overflow
and collisions before mutation. Argument graphs reject foreign references and
unsupported cycles; JS result cycles return explicit errors.

Production pending requests execute JS operations outside the session borrow.
ScriptRef proxies use the available registry during conversion, and nested runtime
calls share the outer instruction budget while restoring call depth on success
and error. Method calls preserve the interpreted receiver.

String and StringChunk value evaluation uses a separate owned continuation,
preserving its parent's frames and operands. It retains legacy constructor and
fallback behavior, validates StringChunk sources, and writes indexed collection
changes back to their original JS property. A production regression exercises
`g = h.factory()`, followed by both `"g.x".value` and `value("g.x")`.

Verification: **536 native library tests**, WASM test compilation, **34 frontend
tests**, and **13 existing browser fixtures plus one new JS ownership fixture**
passed. The browser fixture executes get/set/call through the production pending
executor and includes interpreted JS. The final 374-file source/e2e manifest was
checked after validation. The pinned Ruffle revision, seven preexisting Ruffle
edits, and Stage 1 mise configuration were preserved.

Browser/frontend gates ran before the final two native coverage additions. The
only later changes are the already test-gated `interp_tests.rs`, a `#[cfg(test)]`
interpreter state accessor, and an additional test inside the loader's
`#[cfg(test)]` module. The navigator reconstructed the previous interpreter and
loader bytes by removing these additions and matched their recorded browser
hashes exactly. See [the verified test-only delta](browser-test-only-delta.patch),
[the browser manifest](browser-source-manifest.tsv), and
[the final manifest](runtime-source-manifest.tsv). Native and WASM gates were
refreshed afterward.

See [commands and validation scope](validation-receipt.txt) and
[relevant executed tests](native-relevant-results.txt). Earlier failed builds,
the detected evaluator requeue hang, and failing fixtures were repaired; they
are not acceptance evidence. The regression that exposed the hang now completes
through the real owner scheduler.

Remaining work includes the full direct-consumer/static-helper audit and the
plan placement of session-level seed selection. General caller-dropped evaluator
future cleanup and malformed public completion-route handling remain Stage 2.3
audit items. Arbitrary cyclic/alias-preserving cross-language graph copying is
not implemented; current cycles produce controlled errors. No whole-Stage-2 or
working-native-player claim is made by this checkpoint.
