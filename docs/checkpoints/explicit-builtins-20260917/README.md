# Explicit built-in dispatch component

Accepted bounded Stage 2.2 component: `SpriteBox`, `PuppetTransition` and
`Preload` now use the passed execution context, with owned-reference validation
before mutation. Only consumed arguments are validated; numeric defaults, clamps,
label resolution and ignored arguments retain their existing behavior.

Source: isolated `d8f868fd` plus the accepted three allocator regressions and
`handlers/manager.rs` SHA-256
`8a3cbb96178a85439f67d332bd066633e5cff9a16e8411a7378b45feb915bd4e`.
All 419 source hashes were rechecked after verification, and the live manager
file matches this hash. Concurrent child Flash work is excluded.

Fresh native compilation succeeded. The three focused tests pass; the exact
resulting executable passes **576 tests, 0 failures**. WASM `cargo check --tests`
passes. Commands, source hashes, native artifact identity and output are retained
here. No browser run is claimed for this isolated component.

Tests exercise selected-player geometry/transition/frame lookup, ignored foreign
arguments, and consumed foreign/stale references rejected before mutation. The
full suite retains existing driver/session precedence and error/depth cleanup
regressions. Source review confirms the three branches no longer select ambient
player state.

This does not close Stage 2.2: other ambient global/datum/service branches and
production compiled-runner integration remain open. The combined browser gate
and all remaining Stage 2 ownership requirements still apply.
