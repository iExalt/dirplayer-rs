# JS RNG ownership checkpoint — 2026-09-17

Accepted RNG component of Stage 2.8; generic JS object operations, retained
object capabilities and the full Stage 2.8 gate remain open.

Each `JsRuntime` owns its `Math.random` state. The installed native function
captures that state instead of consulting thread-local storage. The xorshift
algorithm and default seed are unchanged. Explicit seeded construction and
reseed APIs reject zero before changing state. No runtime or bridge borrow is
needed to advance the captured RNG state.

Verification: **508 native library tests passed, 0 failed**, including five
installed-`Math.random` tests for exact default values, same/different seeds,
interleaving, rejected reseeding, replay and output range. WASM test compilation
passed. The navigator independently checked the default sequence and both
approved file hashes. Comparison against the accepted typed-mutation source
manifest found only the two approved RNG changes.

See [commands, artifact hash and validation scope](validation-receipt.txt),
[executed RNG tests](native-relevant-results.txt), and
[final runtime source manifest](runtime-source-manifest.tsv). No mid-run whole
source snapshot was captured; the final manifest and file hashes describe
the source inspected after validation. No other runtime source writer was active.

Session-level seed selection is not wired by this component. Existing callers
use the deterministic default; the new explicit APIs support subsequent service
integration. No browser/host ABI changed, so no browser suite was repeated.
