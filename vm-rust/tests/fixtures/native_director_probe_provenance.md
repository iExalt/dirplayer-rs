# Native Director probe provenance

`generate_native_director_probe.py` emits `native_director_probe.dcr` from
literal D5 records. It contains a DRCF configuration, a KEY* table, one
CAS*/CASt QuickDraw shape, one movie-script CASt, Lctx/Lnam/Lscr script
context, and a single VWSC score frame. It has no external casts, Flash,
audio, fonts, nested movies, or Xtras.

The CASt record uses raw D5 member type 8 with a 17-byte ShapeInfo payload;
the native parser's shape disambiguation is the behavior under test. The
score places cast member 1 at (0,0)-(32,32), so a successful native snapshot
must contain nontransparent pixels in the 32x32 stage. The four authored
handlers (`prepareMovie`, `startMovie`, `enterFrame`, and `exitFrame`) each
write 1 to a distinct phase global; the test asserts all four after
`init_movie`, proving the native startup event sequence. Calling `step_frame`
is intentionally outside the success assertion because the current TestPlayer
has no renderer binding and returns `owned player has no renderer` at that
boundary; its real-tempo sleep is unchanged.

The missing-resource follow-up should pass a missing primary DCR path to the
same local loader and record the expected `std::fs::read` boundary. It is kept
separate from this parser-valid proof so a missing file cannot be confused
with fixture parsing or native host behavior.

Regenerate and validate with:

```sh
mise exec -- python3 vm-rust/tests/fixtures/generate_native_director_probe.py
mise exec -- python3 vm-rust/tests/fixtures/validate_native_director_probe.py
```

The generator is deterministic; the validator prints the SHA-256 used by the
probe evidence receipt.
