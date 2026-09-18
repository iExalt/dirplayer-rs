# Native Director probe provenance

`generate_native_director_probe.py` emits `native_director_probe.dcr` from
literal D5 records. It contains a DRCF configuration, a KEY* table, one
CAS*/CASt QuickDraw shape, one movie-script CASt, Lctx/Lnam/Lscr script
context, and a single VWSC score frame. It has no external casts, Flash,
audio, fonts, nested movies, or Xtras.

The CASt record uses raw D5 member type 8 with a 17-byte ShapeInfo payload;
the native parser's shape disambiguation is the behavior under test. CAS* maps
movie-script member 1 before QuickDraw shape member 2, and the score places
member 2 at position `(4,4)` with size `24x24`, rendering the inset rect
`(4,4)-(28,28)`. Its score foreground is palette index 255 (black) and its
background is index 0 (white). A successful native snapshot therefore has a
white `(0,0)` corner, a black `(16,16)` center, and exactly 576 opaque black
pixels. The four authored handlers (`prepareMovie`, `startMovie`, `enterFrame`,
and `exitFrame`) each write 1 to a distinct phase global; the test asserts all
four after `init_movie`, proving the native startup event sequence. Calling
`step_frame` is intentionally outside the success assertion because the
current TestPlayer has no renderer binding and returns `owned player has no
renderer` at that boundary; its real-tempo sleep is unchanged.

The first contrast encoding swapped CAS* and inset geometry but left the D5
foreground byte at palette index 0, so the focused probe correctly observed a
white center and produced no PNG. The bounded second encoding set the authored
foreground to palette index 255 and encoded the inset as 24x24; it passed with
the exact corner, center, and 576-pixel assertions. This was a fixture color
mapping correction, not a native-host blocker.

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
