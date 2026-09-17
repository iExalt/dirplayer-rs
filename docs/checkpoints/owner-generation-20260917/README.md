# Player owner-generation lifecycle

Accepted component based on `243647a3`, published with evaluator cancellation.

RuntimeSession retains the highest generation issued for each player ID across
removal, observes direct player resets before removal, and advances replacement
generations without wrapping. Session reset rejects exhausted generations before
cancelling work. Infallible player/allocator reset APIs panic on exhaustion before
mutating state. Other player IDs and the session symbol owner are unchanged.

The recorded source passes 561 native tests, WASM test compilation and the
combined 15-fixture browser gate. Both focused Flash browser tests also pass. Focused
tests cover removal while retaining the old player, reset then replacement,
sibling isolation, direct reset, and exhaustion at session/player/allocator
boundaries. The source hashes identify the four changed Rust files against the
base commit; they are not a manifest of unrelated Ruffle working-tree changes.

The authored SWF fixture proves initial Flash access and reset through the
existing owner tombstone protections. This closes the bounded root initial-access
component, not Stage 2.6 or Stage 2: child Flash host lifecycle remains open.
